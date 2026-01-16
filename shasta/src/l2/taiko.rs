//TODO remove
#![allow(dead_code)]

use super::execution_layer::L2ExecutionLayer;
use crate::l1::protocol_config::ProtocolConfig;
use crate::node::proposal_manager::l2_block_payload::L2BlockV2Payload;
use alloy::{
    consensus::BlockHeader,
    eips::BlockNumberOrTag,
    primitives::{Address, B256},
    rpc::types::Block,
};
use anyhow::Error;
use common::shared::l2_slot_info_v2::L2SlotContext;
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
        l2_slot_info_v2::L2SlotInfoV2,
        l2_tx_lists::{self, PreBuiltTxList},
    },
};
use pacaya::l2::config::TaikoConfig;
use std::{sync::Arc, time::Duration};
use taiko_alethia_reth::validation::ANCHOR_V3_V4_GAS_LIMIT;
use taiko_bindings::anchor::{Anchor, ICheckpointStore::Checkpoint};
use tracing::{debug, trace};

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

    pub async fn get_l2_slot_info(&self) -> Result<L2SlotInfoV2, Error> {
        self.get_l2_slot_info_by_parent_block(BlockNumberOrTag::Latest)
            .await
    }

    pub async fn get_l2_slot_info_by_parent_block(
        &self,
        parent: BlockNumberOrTag,
    ) -> Result<L2SlotInfoV2, Error> {
        let l2_slot_timestamp = self.slot_clock.get_l2_slot_begin_timestamp()?;
        let parent_block = self
            .l2_execution_layer
            .common()
            .get_block_header(parent)
            .await?;
        let parent_id = parent_block.header.number();
        let parent_hash = parent_block.header.hash;
        let parent_gas_limit = parent_block.header.gas_limit();
        let parent_timestamp = parent_block.header.timestamp();

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

        let base_fee: u64 = self.get_base_fee(parent_block).await?;

        trace!(
            timestamp = %l2_slot_timestamp,
            parent_hash = %parent_hash,
            parent_gas_limit_without_anchor = %parent_gas_limit_without_anchor,
            parent_timestamp = %parent_timestamp,
            base_fee = %base_fee,
            "L2 slot info"
        );

        Ok(L2SlotInfoV2::new(
            base_fee,
            l2_slot_timestamp,
            parent_id,
            parent_hash,
            parent_gas_limit_without_anchor,
            parent_timestamp,
        ))
    }

    async fn get_base_fee(&self, parent_block: Block) -> Result<u64, Error> {
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
        l2_block_payload: L2BlockV2Payload,
        l2_slot_context: &L2SlotContext,
        operation_type: OperationType,
    ) -> Result<BuildPreconfBlockResponse, Error> {
        tracing::debug!(
            "Submitting new L2 block to the Taiko driver with {} txs",
            l2_block_payload.tx_list.len()
        );

        let anchor_block_params = Checkpoint {
            blockNumber: l2_block_payload.anchor_block_id.try_into()?,
            blockHash: l2_block_payload.anchor_block_hash,
            stateRoot: l2_block_payload.anchor_state_root,
        };

        let anchor_tx = self
            .l2_execution_layer
            .construct_anchor_tx(&l2_slot_context.info, anchor_block_params)
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "advance_head_to_new_l2_block: Failed to construct anchor tx: {}",
                    e
                )
            })?;
        let tx_list = std::iter::once(anchor_tx)
            .chain(l2_block_payload.tx_list.into_iter())
            .collect::<Vec<_>>();

        let tx_list_bytes = l2_tx_lists::encode_and_compress(&tx_list)?;

        let sharing_pctg = self.protocol_config.get_basefee_sharing_pctg();
        let extra_data = super::extra_data::ExtraData {
            basefee_sharing_pctg: sharing_pctg,
            proposal_id: l2_block_payload.proposal_id,
        }
        .encode()
        .map_err(|e| {
            anyhow::anyhow!(
                "advance_head_to_new_l2_block: Failed to encode extra data: {}",
                e
            )
        })?;

        let executable_data = ExecutableData {
            base_fee_per_gas: l2_slot_context.info.base_fee(),
            block_number: l2_slot_context.info.parent_id() + 1,
            extra_data: format!("0x{}", hex::encode(extra_data)),
            fee_recipient: l2_block_payload.coinbase.to_string(),
            gas_limit: l2_block_payload.gas_limit_without_anchor + ANCHOR_V3_V4_GAS_LIMIT,
            parent_hash: format!("0x{}", hex::encode(l2_slot_context.info.parent_hash())),
            timestamp: l2_block_payload.timestamp_sec,
            transactions: format!("0x{}", hex::encode(tx_list_bytes)),
        };

        let request_body = BuildPreconfBlockRequestBody {
            executable_data,
            end_of_sequencing: l2_slot_context.end_of_sequencing,
            is_forced_inclusion: l2_block_payload.is_forced_inclusion,
        };

        self.driver
            .preconf_blocks(request_body, operation_type)
            .await
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
