//TODO remove
#![allow(dead_code)]

use super::execution_layer::L2ExecutionLayer;
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
        taiko_driver::{TaikoDriver, TaikoDriverConfig, models::TaikoStatus},
        traits::Bridgeable,
    },
    metrics::Metrics,
    shared::l2_tx_lists::PreBuiltTxList,
};
use pacaya::l1::protocol_config::ProtocolConfig;
use pacaya::l2::config::TaikoConfig;
use std::{sync::Arc, time::Duration};
use tracing::debug;

pub struct Taiko {
    protocol_config: ProtocolConfig,
    l2_execution_layer: Arc<L2ExecutionLayer>,
    driver: TaikoDriver,
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
            call_timeout: Duration::from_secs(taiko_config.preconf_heartbeat_ms / 2),
        };
        Ok(Self {
            protocol_config,
            l2_execution_layer: Arc::new(
                L2ExecutionLayer::new(taiko_config.clone())
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to create L2ExecutionLayer: {}", e))?,
            ),
            driver: TaikoDriver::new(&driver_config, metrics).await?,
            slot_clock,
            coinbase: format!("0x{}", hex::encode(taiko_config.signer.get_address())),
            l2_engine,
        })
    }

    pub fn l2_execution_layer(&self) -> Arc<L2ExecutionLayer> {
        self.l2_execution_layer.clone()
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
                self.get_protocol_config().get_block_max_gas_limit(),
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

    pub async fn get_l2_block_id_hash_and_gas_used(
        &self,
        block: BlockNumberOrTag,
    ) -> Result<(u64, B256, u64), Error> {
        let block = self
            .l2_execution_layer
            .common()
            .get_block_header(block)
            .await?;

        Ok((
            block.header.number(),
            block.header.hash,
            block.header.gas_used(),
        ))
    }

    pub async fn get_l2_block_hash(&self, number: u64) -> Result<B256, Error> {
        self.l2_execution_layer
            .common()
            .get_block_hash(number)
            .await
    }

    pub async fn get_status(&self) -> Result<TaikoStatus, Error> {
        self.driver.get_status().await
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

pub trait PreconfDriver {
    fn get_status(&self) -> impl std::future::Future<Output = Result<TaikoStatus, Error>> + Send;
}

impl PreconfDriver for Taiko {
    async fn get_status(&self) -> Result<TaikoStatus, Error> {
        Taiko::get_status(self).await
    }
}
