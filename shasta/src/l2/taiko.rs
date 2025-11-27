//TODO remove
#![allow(dead_code)]

use super::execution_layer::L2ExecutionLayer;
use crate::l1::protocol_config::ProtocolConfig;
use crate::node::proposal_manager::proposal::BondInstructionData;
use crate::node::proposal_manager::proposal::Proposal;
use alloy::{
    consensus::BlockHeader,
    eips::BlockNumberOrTag,
    primitives::{Address, B256},
    rpc::types::Transaction,
};
use anyhow::Error;
use common::{
    l1::slot_clock::SlotClock,
    l2::{
        engine::L2Engine,
        taiko_driver::{
            OperationType, TaikoDriver, TaikoDriverConfig,
            models::{BuildPreconfBlockRequestBody, BuildPreconfBlockResponse, ExecutableData},
        },
        traits::Bridgeable,
    },
    metrics::Metrics,
    shared::{
        l2_slot_info::L2SlotInfo,
        l2_tx_lists::{self, PreBuiltTxList},
    },
};
use pacaya::l2::config::TaikoConfig;
use std::{sync::Arc, time::Duration};
use taiko_bindings::anchor::Anchor;
use tracing::{debug, trace};

// TODO: retrieve from protocol
const ANCHOR_V3_V4_GAS_LIMIT: u64 = 1_000_000;

pub struct Taiko {
    protocol_config: ProtocolConfig,
    l2_execution_layer: Arc<L2ExecutionLayer>,
    driver: Arc<TaikoDriver>,
    slot_clock: Arc<SlotClock>,
    coinbase: String,
    l2_engine: L2Engine,
}

impl Taiko {
    pub async fn new(
        slot_clock: Arc<SlotClock>,
        protocol_config: ProtocolConfig,
        metrics: Arc<Metrics>,
        taiko_config: TaikoConfig,
        l2_engine: L2Engine,
    ) -> Result<Self, Error> {
        let driver_config: TaikoDriverConfig = TaikoDriverConfig {
            driver_url: taiko_config.driver_url.clone(),
            rpc_driver_preconf_timeout: taiko_config.rpc_driver_preconf_timeout,
            rpc_driver_status_timeout: taiko_config.rpc_driver_status_timeout,
            jwt_secret_bytes: taiko_config.jwt_secret_bytes,
            call_timeout: Duration::from_millis(taiko_config.preconf_heartbeat_ms / 2),
        };
        Ok(Self {
            protocol_config,
            l2_execution_layer: Arc::new(
                L2ExecutionLayer::new(taiko_config.clone())
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to create L2ExecutionLayer: {}", e))?,
            ),
            driver: Arc::new(TaikoDriver::new(&driver_config, metrics).await?),
            slot_clock,
            coinbase: format!("0x{}", hex::encode(taiko_config.signer.get_address())),
            l2_engine,
        })
    }

    pub fn get_driver(&self) -> Arc<TaikoDriver> {
        self.driver.clone()
    }

    pub fn l2_execution_layer(&self) -> Arc<L2ExecutionLayer> {
        self.l2_execution_layer.clone()
    }

    pub async fn get_pending_l2_tx_list_from_l2_engine(
        &self,
        base_fee: u64,
        batches_ready_to_send: u64,
        gas_limit: u64,
    ) -> Result<Option<PreBuiltTxList>, Error> {
        self.l2_engine
            .get_pending_l2_tx_list(base_fee, batches_ready_to_send, gas_limit)
            .await
    }

    pub fn get_protocol_config(&self) -> &ProtocolConfig {
        &self.protocol_config
    }

    pub async fn get_latest_l2_block_id(&self) -> Result<u64, Error> {
        self.l2_execution_layer.common().get_latest_block_id().await
    }

    pub async fn get_l2_block_by_number(
        &self,
        number: u64,
        full_txs: bool,
    ) -> Result<alloy::rpc::types::Block, Error> {
        self.l2_execution_layer
            .common()
            .get_block_by_number(number, full_txs)
            .await
    }

    pub async fn fetch_l2_blocks_until_latest(
        &self,
        start_block: u64,
        full_txs: bool,
    ) -> Result<Vec<alloy::rpc::types::Block>, Error> {
        let start_time = std::time::Instant::now();
        let end_block = self.get_latest_l2_block_id().await?;
        let mut blocks = Vec::with_capacity(usize::try_from(end_block - start_block + 1)?);
        for block_number in start_block..=end_block {
            let block = self.get_l2_block_by_number(block_number, full_txs).await?;
            blocks.push(block);
        }
        debug!(
            "Fetched L2 blocks from {} to {} in {} ms",
            start_block,
            end_block,
            start_time.elapsed().as_millis()
        );
        Ok(blocks)
    }

    pub async fn get_transaction_by_hash(
        &self,
        hash: B256,
    ) -> Result<alloy::rpc::types::Transaction, Error> {
        self.l2_execution_layer
            .common()
            .get_transaction_by_hash(hash)
            .await
    }

    pub async fn get_l2_block_hash(&self, number: u64) -> Result<B256, Error> {
        self.l2_execution_layer
            .common()
            .get_block_hash(number)
            .await
    }

    pub async fn get_l2_slot_info(&self) -> Result<L2SlotInfo, Error> {
        self.get_l2_slot_info_by_parent_block(BlockNumberOrTag::Latest)
            .await
    }

    pub async fn get_l2_slot_info_by_parent_block(
        &self,
        block: BlockNumberOrTag,
    ) -> Result<L2SlotInfo, Error> {
        let l2_slot_timestamp = self.slot_clock.get_l2_slot_begin_timestamp()?;
        let block_info = self
            .l2_execution_layer
            .common()
            .get_block_header(block)
            .await?;
        let parent_id = block_info.header.number();
        let parent_hash = block_info.header.hash;
        let parent_gas_used = block_info.header.gas_used();
        let parent_gas_limit = block_info.header.gas_limit();
        let parent_timestamp = block_info.header.timestamp();

        let parent_gas_limit_without_anchor = if parent_id != 0 {
            parent_gas_limit
                .checked_sub(ANCHOR_V3_V4_GAS_LIMIT)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "parent_gas_limit {} is less than ANCHOR_V3_V4_GAS_LIMIT {}",
                        parent_gas_limit,
                        ANCHOR_V3_V4_GAS_LIMIT
                    )
                })?
        } else {
            parent_gas_limit
        };

        // Safe conversion with overflow check
        let parent_gas_used_u32 = u32::try_from(parent_gas_used).map_err(|_| {
            anyhow::anyhow!("parent_gas_used {} exceeds u32 max value", parent_gas_used)
        })?;

        let base_fee: u64 = self.get_base_fee(block).await?;

        trace!(
            timestamp = %l2_slot_timestamp,
            parent_hash = %parent_hash,
            parent_gas_used = %parent_gas_used_u32,
            parent_gas_limit_without_anchor = %parent_gas_limit_without_anchor,
            parent_timestamp = %parent_timestamp,
            base_fee = %base_fee,
            "L2 slot info"
        );

        Ok(L2SlotInfo::new(
            base_fee,
            l2_slot_timestamp,
            parent_id,
            parent_hash,
            parent_gas_used_u32,
            parent_gas_limit_without_anchor,
            parent_timestamp,
        ))
    }

    async fn get_base_fee(&self, block: BlockNumberOrTag) -> Result<u64, Error> {
        let parent_block = self
            .l2_execution_layer
            .common()
            .get_block_header(block)
            .await?;

        if parent_block.header.number() == 0 {
            return Ok(taiko_alethia_reth::eip4396::SHASTA_INITIAL_BASE_FEE);
        }

        let grandparent_number = parent_block.header.number() - 1;
        let grandparent_timestamp = self
            .l2_execution_layer
            .common()
            .get_block_header(BlockNumberOrTag::Number(grandparent_number))
            .await?
            .header
            .timestamp();

        let timestamp_diff = parent_block
            .header
            .timestamp()
            .checked_sub(grandparent_timestamp)
            .ok_or_else(|| anyhow::anyhow!("Timestamp underflow occurred"))?;

        let base_fee = taiko_alethia_reth::eip4396::calculate_next_block_eip4396_base_fee(
            &parent_block.header.inner,
            timestamp_diff,
        );

        Ok(base_fee)
    }

    // TODO fix that function
    #[allow(clippy::too_many_arguments)]
    pub async fn advance_head_to_new_l2_block(
        &self,
        proposal: &Proposal,
        l2_slot_info: &L2SlotInfo,
        tx_list: Vec<Transaction>,
        end_of_sequencing: bool,
        is_forced_inclusion: bool,
        operation_type: OperationType,
    ) -> Result<Option<BuildPreconfBlockResponse>, Error> {
        tracing::debug!(
            "Submitting new L2 block to the Taiko driver with {} txs",
            tx_list.len()
        );

        let timestamp = if is_forced_inclusion {
            l2_slot_info.parent_timestamp() + 1
        } else {
            proposal.get_last_block_timestamp()?
        };

        let anchor_block_params = if is_forced_inclusion {
            self.l2_execution_layer
                .get_last_synced_block_params_from_geth()
                .await?
        } else {
            Anchor::BlockParams {
                anchorBlockNumber: proposal.anchor_block_id.try_into()?,
                anchorBlockHash: proposal.anchor_block_hash,
                anchorStateRoot: proposal.anchor_state_root,
            }
        };

        let use_full_instructions = (is_forced_inclusion && proposal.is_empty())
            || (!is_forced_inclusion && proposal.has_only_one_common_block());

        let bond_instructions = if use_full_instructions {
            BondInstructionData::new(
                proposal.bond_instructions.instructions().clone(),
                proposal.bond_instructions.hash(),
            )
        } else {
            BondInstructionData::new(vec![], proposal.bond_instructions.hash())
        };

        let anchor_tx = self
            .l2_execution_layer
            .construct_anchor_tx(
                proposal.id,
                l2_slot_info,
                anchor_block_params,
                bond_instructions,
            )
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "advance_head_to_new_l2_block: Failed to construct anchor tx: {}",
                    e
                )
            })?;
        let tx_list = std::iter::once(anchor_tx)
            .chain(tx_list.into_iter())
            .collect::<Vec<_>>();

        let tx_list_bytes = l2_tx_lists::encode_and_compress(&tx_list)?;

        let sharing_pctg = self.protocol_config.get_basefee_sharing_pctg();
        let extra_data = Self::encode_extra_data(sharing_pctg, false);

        let executable_data = ExecutableData {
            base_fee_per_gas: l2_slot_info.base_fee(),
            block_number: l2_slot_info.parent_id() + 1,
            extra_data: format!("0x{:04x}", extra_data),
            fee_recipient: proposal.coinbase.to_string(),
            gas_limit: l2_slot_info.parent_gas_limit_without_anchor() + ANCHOR_V3_V4_GAS_LIMIT,
            parent_hash: format!("0x{}", hex::encode(l2_slot_info.parent_hash())),
            timestamp,
            transactions: format!("0x{}", hex::encode(tx_list_bytes)),
        };

        let request_body = BuildPreconfBlockRequestBody {
            executable_data,
            end_of_sequencing,
            is_forced_inclusion,
        };

        self.driver
            .preconf_blocks(request_body, operation_type)
            .await
    }

    fn encode_extra_data(basefee_sharing_pctg: u8, is_low_bond_proposal: bool) -> u16 {
        u16::from(basefee_sharing_pctg) << 8 | u16::from(is_low_bond_proposal)
    }

    pub fn decode_anchor_id_from_tx_data(data: &[u8]) -> Result<u64, Error> {
        L2ExecutionLayer::decode_anchor_id_from_tx_data(data)
    }

    pub fn get_anchor_tx_data(data: &[u8]) -> Result<Anchor::anchorV4Call, Error> {
        L2ExecutionLayer::get_anchor_tx_data(data)
    }

    pub async fn get_forced_inclusion_form_l1origin(&self, block_id: u64) -> Result<bool, Error> {
        self.l2_execution_layer
            .get_forced_inclusion_form_l1origin(block_id)
            .await
    }
}

impl Bridgeable for Taiko {
    async fn get_balance(&self, address: Address) -> Result<alloy::primitives::U256, Error> {
        self.l2_execution_layer
            .common()
            .get_account_balance(address)
            .await
    }

    async fn transfer_eth_from_l2_to_l1(
        &self,
        amount: u128,
        dest_chain_id: u64,
        address: Address,
        bridge_relayer_fee: u64,
    ) -> Result<(), Error> {
        self.l2_execution_layer
            .transfer_eth_from_l2_to_l1(amount, dest_chain_id, address, bridge_relayer_fee)
            .await
    }
}

mod tests {
    #[test]
    fn test_encode_extra_data() {
        use super::Taiko;

        let extra_data = Taiko::encode_extra_data(30, true);
        assert_eq!(extra_data, 0b00011110_00000001);

        let extra_data = Taiko::encode_extra_data(50, false);
        assert_eq!(extra_data, 0b00110010_00000000);
    }
}
