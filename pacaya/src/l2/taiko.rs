use super::{
    bindings::TaikoAnchor::BaseFeeConfig, config::TaikoConfig, execution_layer::L2ExecutionLayer,
};
use crate::l1::protocol_config::ProtocolConfig;
use alloy::{
    consensus::BlockHeader,
    eips::BlockNumberOrTag,
    primitives::{Address, B256},
};
use anyhow::Error;
use common::{
    l1::slot_clock::SlotClock,
    l2::engine::L2Engine,
    l2::{
        taiko_driver::{
            OperationType, TaikoDriver, TaikoDriverConfig,
            models::{BuildPreconfBlockRequestBody, BuildPreconfBlockResponse, ExecutableData},
        },
        traits::Bridgeable,
    },
    metrics::Metrics,
    shared::{
        l2_block::L2Block,
        l2_slot_info::L2SlotInfo,
        l2_tx_lists::{self, PreBuiltTxList},
    },
};
use std::{sync::Arc, time::Duration};
use tracing::{debug, trace};

pub struct Taiko {
    protocol_config: ProtocolConfig,
    l2_execution_layer: L2ExecutionLayer,
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
            l2_execution_layer: L2ExecutionLayer::new(taiko_config.clone())
                .await
                .map_err(|e| anyhow::anyhow!("Failed to create L2ExecutionLayer: {}", e))?,
            driver: Arc::new(TaikoDriver::new(&driver_config, metrics).await?),
            slot_clock,
            coinbase: format!("0x{}", hex::encode(taiko_config.signer.get_address())),
            l2_engine,
        })
    }

    pub fn get_driver(&self) -> Arc<TaikoDriver> {
        self.driver.clone()
    }

    pub async fn get_pending_l2_tx_list_from_l2_engine(
        &self,
        base_fee: u64,
        batches_ready_to_send: u64,
    ) -> Result<Option<PreBuiltTxList>, Error> {
        self.l2_engine
            .get_pending_l2_tx_list(
                base_fee,
                batches_ready_to_send,
                self.get_protocol_config().get_block_max_gas_limit().into(),
            )
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

    pub async fn get_forced_inclusion_form_l1origin(&self, block_id: u64) -> Result<bool, Error> {
        self.l2_execution_layer
            .get_forced_inclusion_form_l1origin(block_id)
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
        let parent_timestamp = block_info.header.timestamp();
        // Safe conversion with overflow check
        let parent_gas_used_u32 = u32::try_from(parent_gas_used).map_err(|_| {
            anyhow::anyhow!("parent_gas_used {} exceeds u32 max value", parent_gas_used)
        })?;

        let base_fee_config = self.get_base_fee_config();

        let base_fee = self
            .get_base_fee(
                parent_hash,
                parent_gas_used_u32,
                base_fee_config,
                l2_slot_timestamp,
            )
            .await?;

        trace!(
            timestamp = %l2_slot_timestamp,
            parent_hash = %parent_hash,
            parent_gas_used = %parent_gas_used_u32,
            base_fee = %base_fee,
            "L2 slot info"
        );

        Ok(L2SlotInfo::new(
            base_fee,
            l2_slot_timestamp,
            parent_id,
            parent_hash,
            parent_gas_used_u32,
            parent_timestamp,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn advance_head_to_new_l2_block(
        &self,
        l2_block: L2Block,
        anchor_origin_height: u64,
        anchor_block_state_root: B256,
        l2_slot_info: &L2SlotInfo,
        end_of_sequencing: bool,
        is_forced_inclusion: bool,
        operation_type: OperationType,
    ) -> Result<BuildPreconfBlockResponse, Error> {
        tracing::debug!(
            "Submitting new L2 block to the Taiko driver with {} txs",
            l2_block.prebuilt_tx_list.tx_list.len()
        );

        let base_fee_config = self.get_base_fee_config();
        let sharing_pctg = base_fee_config.sharingPctg;

        let anchor_tx = self
            .l2_execution_layer
            .construct_anchor_tx(
                l2_slot_info,
                anchor_origin_height,
                anchor_block_state_root,
                base_fee_config.clone(),
            )
            .await?;
        let tx_list = std::iter::once(anchor_tx)
            .chain(l2_block.prebuilt_tx_list.tx_list.into_iter())
            .collect::<Vec<_>>();

        let tx_list_bytes = l2_tx_lists::encode_and_compress(&tx_list)?;
        let extra_data = vec![sharing_pctg];

        let executable_data = ExecutableData {
            base_fee_per_gas: l2_slot_info.base_fee(),
            block_number: l2_slot_info.parent_id() + 1,
            extra_data: format!("0x{:0>64}", hex::encode(extra_data)),
            fee_recipient: self.coinbase.clone(),
            gas_limit: 241_000_000u64,
            parent_hash: format!("0x{}", hex::encode(l2_slot_info.parent_hash())),
            timestamp: l2_block.timestamp_sec,
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

    fn get_base_fee_config(&self) -> BaseFeeConfig {
        BaseFeeConfig {
            adjustmentQuotient: self.protocol_config.get_base_fee_adjustment_quotient(),
            sharingPctg: self.protocol_config.get_base_fee_sharing_pctg(),
            gasIssuancePerSecond: self.protocol_config.get_base_fee_gas_issuance_per_second(),
            minGasExcess: self.protocol_config.get_base_fee_min_gas_excess(),
            maxGasIssuancePerBlock: self
                .protocol_config
                .get_base_fee_max_gas_issuance_per_block(),
        }
    }

    pub async fn get_base_fee(
        &self,
        parent_hash: B256,
        parent_gas_used: u32,
        base_fee_config: BaseFeeConfig,
        l2_slot_timestamp: u64,
    ) -> Result<u64, Error> {
        self.l2_execution_layer
            .get_base_fee(
                parent_hash,
                parent_gas_used,
                base_fee_config,
                l2_slot_timestamp,
            )
            .await
    }

    pub async fn get_last_synced_anchor_block_id_from_taiko_anchor(&self) -> Result<u64, Error> {
        self.l2_execution_layer
            .get_last_synced_anchor_block_id_from_taiko_anchor()
            .await
    }

    pub async fn get_last_synced_anchor_block_id_from_geth(&self) -> Result<u64, Error> {
        self.l2_execution_layer
            .get_last_synced_anchor_block_id_from_geth()
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

pub fn decode_anchor_id_from_tx_data(data: &[u8]) -> Result<u64, Error> {
    L2ExecutionLayer::decode_anchor_id_from_tx_data(data)
        .map_err(|e| anyhow::anyhow!("Failed to decode anchor id from tx data: {}", e))
}
