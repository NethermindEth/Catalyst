use super::execution_layer::L2ExecutionLayer;
use crate::forced_inclusion::InboxForcedInclusionState;
use crate::l1::protocol_config::ProtocolConfig;
use alloy::{
    consensus::BlockHeader,
    eips::BlockNumberOrTag,
    primitives::{Address, B256},
    rpc::types::Block,
};
use anyhow::Error;
use common::{
    l1::slot_clock::SlotClock,
    l2::{
        engine::L2Engine,
        taiko_driver::{TaikoDriver, TaikoDriverConfig},
        traits::Bridgeable,
    },
    metrics::Metrics,
    shared::{l2_slot_info_v2::L2SlotInfoV2, l2_tx_lists::PreBuiltTxList},
};
use pacaya::l2::config::TaikoConfig;
use std::sync::Arc;
use taiko_alethia_reth::validation::ANCHOR_V3_V4_GAS_LIMIT;
use taiko_bindings::anchor::Anchor;
use taiko_bindings::inbox::IInbox::Config;
use taiko_protocol::shasta::constants::min_base_fee_for_chain;
use tracing::{debug, trace};

pub struct Taiko {
    protocol_config: ProtocolConfig,
    l2_execution_layer: Arc<L2ExecutionLayer>,
    driver: Arc<TaikoDriver>,
    slot_clock: Arc<SlotClock>,
    l2_engine: L2Engine,
}

impl Taiko {
    pub async fn new(
        slot_clock: Arc<SlotClock>,
        inbox_config: Config,
        metrics: Arc<Metrics>,
        taiko_config: TaikoConfig,
        l2_engine: L2Engine,
    ) -> Result<Self, Error> {
        let driver_config: TaikoDriverConfig = TaikoDriverConfig {
            driver_url: taiko_config.driver_url.clone(),
            rpc_driver_preconf_timeout: taiko_config.rpc_driver_preconf_timeout,
            rpc_driver_status_timeout: taiko_config.rpc_driver_status_timeout,
            rpc_driver_retry_timeout: taiko_config.rpc_driver_retry_timeout,
            jwt_secret_bytes: taiko_config.jwt_secret_bytes,
        };

        let l2_execution_layer = Arc::new(
            L2ExecutionLayer::new(taiko_config.clone())
                .await
                .map_err(|e| anyhow::anyhow!("Failed to create L2ExecutionLayer: {}", e))?,
        );
        let protocol_config =
            ProtocolConfig::from(l2_execution_layer.common().chain_id(), &inbox_config);
        Ok(Self {
            protocol_config,
            l2_execution_layer,
            driver: Arc::new(TaikoDriver::new(&driver_config, metrics).await?),
            slot_clock,
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
        proposals_ready_to_send: u64,
        gas_limit: u64,
    ) -> Result<Option<PreBuiltTxList>, Error> {
        self.l2_engine
            .get_pending_l2_tx_list(base_fee, proposals_ready_to_send, gas_limit)
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

    pub async fn calculate_current_fi_head(
        &self,
        inbox_forced_inclusion_state: InboxForcedInclusionState,
    ) -> Result<u64, Error> {
        let mut fi_head = inbox_forced_inclusion_state.head;
        if inbox_forced_inclusion_state.next_proposal_id > 2
            && fi_head < inbox_forced_inclusion_state.tail
        {
            let start = std::time::Instant::now();
            let safe_block_id = self
                .get_last_block_id_by_proposal_id(inbox_forced_inclusion_state.next_proposal_id - 1)
                .await?;
            let unsafe_block_id = self.get_latest_l2_block_id().await?;
            for block_id in safe_block_id + 1..=unsafe_block_id {
                let is_forced_inclusion = self.get_forced_inclusion_form_l1origin(block_id).await?;
                if is_forced_inclusion {
                    fi_head += 1;
                }
                if fi_head == inbox_forced_inclusion_state.tail {
                    break;
                }
            }
            debug!(
                "Calculated forced inclusion head: {} in {} ms (unsafe head: {}, safe head: {})",
                fi_head,
                start.elapsed().as_millis(),
                unsafe_block_id,
                safe_block_id
            );
        }
        Ok(fi_head)
    }

    pub async fn get_last_block_id_by_proposal_id(&self, proposal_id: u64) -> Result<u64, Error> {
        match self
            .l2_engine
            .get_last_block_id_by_batch_id(proposal_id)
            .await?
        {
            Some(block_id) => Ok(block_id),
            None => Err(anyhow::anyhow!(
                "last block id by proposal id {} is None",
                proposal_id
            )),
        }
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
            .ok_or_else(|| anyhow::anyhow!("get_base_fee:Timestamp underflow occurred"))?;

        let parent_base_fee_per_gas =
            parent_block.header.inner.base_fee_per_gas.ok_or_else(|| {
                anyhow::anyhow!(
                    "get_base_fee: Parent block missing base fee per gas for block {}",
                    parent_block.header.number()
                )
            })?;
        let base_fee = taiko_alethia_reth::eip4396::calculate_next_block_eip4396_base_fee(
            &parent_block.header.inner,
            timestamp_diff,
            parent_base_fee_per_gas,
            min_base_fee_for_chain(self.l2_execution_layer.common().chain_id()),
        );

        Ok(base_fee)
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
