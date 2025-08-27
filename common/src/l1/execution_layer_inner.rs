use crate::{
    l1::{monitor_transaction::TransactionMonitor, transaction_error::TransactionError},
    metrics,
    shared::alloy_tools,
};
use alloy::{
    eips::BlockNumberOrTag,
    primitives::{Address, B256},
    providers::{DynProvider, Provider},
    rpc::types::{Filter, Log},
};
use anyhow::{Error, anyhow};
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tracing::info;

pub struct ExecutionLayerInner {
    pub metrics: Arc<metrics::Metrics>,
    provider: DynProvider,
    chain_id: u64,
    preconfer_address: Address,
    extra_gas_percentage: u64,
    pub transaction_monitor: TransactionMonitor,
}

impl ExecutionLayerInner {
    pub async fn new(
        config_common: super::config::EthereumL1Config,
        transaction_error_channel: Sender<TransactionError>,
        metrics: Arc<metrics::Metrics>,
    ) -> Result<Self, Error> {
        let (provider, preconfer_address) = alloy_tools::construct_alloy_provider(
            &config_common.signer,
            config_common
                .execution_rpc_urls
                .first()
                .ok_or_else(|| anyhow!("L1 RPC URL is required"))?,
            config_common.preconfer_address,
        )
        .await?;
        info!("Catalyst node address: {}", preconfer_address);

        let chain_id = provider
            .get_chain_id()
            .await
            .map_err(|e| Error::msg(format!("Failed to get chain ID: {e}")))?;
        info!("L1 Chain ID: {}", chain_id);

        let transaction_monitor = TransactionMonitor::new(
            provider.clone(),
            &config_common,
            transaction_error_channel,
            metrics.clone(),
            chain_id,
        )
        .await
        .map_err(|e| Error::msg(format!("Failed to create TransactionMonitor: {e}")))?;

        Ok(Self {
            metrics,
            provider,
            chain_id,
            preconfer_address,
            extra_gas_percentage: config_common.extra_gas_percentage,
            transaction_monitor,
        })
    }

    pub fn provider(&self) -> DynProvider {
        self.provider.clone()
    }

    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    pub fn preconfer_address(&self) -> Address {
        self.preconfer_address
    }

    pub fn extra_gas_percentage(&self) -> u64 {
        self.extra_gas_percentage
    }

    pub async fn get_preconfer_nonce_pending(&self) -> Result<u64, Error> {
        let nonce_str: String = self
            .provider
            .client()
            .request(
                "eth_getTransactionCount",
                (self.preconfer_address, "pending"),
            )
            .await
            .map_err(|e| Error::msg(format!("Failed to get nonce: {e}")))?;

        u64::from_str_radix(nonce_str.trim_start_matches("0x"), 16)
            .map_err(|e| Error::msg(format!("Failed to convert nonce: {e}")))
    }

    pub async fn get_l1_height(&self) -> Result<u64, Error> {
        self.provider
            .get_block_number()
            .await
            .map_err(|e| Error::msg(format!("Failed to get L1 height: {e}")))
    }

    pub async fn get_preconfer_wallet_eth(&self) -> Result<alloy::primitives::U256, Error> {
        let balance = self.provider.get_balance(self.preconfer_address).await?;
        Ok(balance)
    }

    pub async fn get_block_state_root_by_number(&self, number: u64) -> Result<B256, Error> {
        let block = self
            .provider
            .get_block_by_number(BlockNumberOrTag::Number(number))
            .await
            .map_err(|e| Error::msg(format!("Failed to get block by number ({number}): {e}")))?
            .ok_or(anyhow::anyhow!("Failed to get block by number ({number})"))?;
        Ok(block.header.state_root)
    }

    async fn get_block_timestamp_by_number_or_tag(
        &self,
        block_number_or_tag: BlockNumberOrTag,
    ) -> Result<u64, Error> {
        let block = self
            .provider
            .get_block_by_number(block_number_or_tag)
            .await?
            .ok_or(anyhow::anyhow!(
                "Failed to get block by number ({})",
                block_number_or_tag
            ))?;
        Ok(block.header.timestamp)
    }

    pub async fn get_preconfer_nonce_latest(&self) -> Result<u64, Error> {
        let nonce_str: String = self
            .provider
            .client()
            .request(
                "eth_getTransactionCount",
                (self.preconfer_address, "latest"),
            )
            .await
            .map_err(|e| Error::msg(format!("Failed to get nonce: {e}")))?;

        u64::from_str_radix(nonce_str.trim_start_matches("0x"), 16)
            .map_err(|e| Error::msg(format!("Failed to convert nonce: {e}")))
    }

    pub async fn get_block_timestamp_by_number(&self, block: u64) -> Result<u64, Error> {
        self.get_block_timestamp_by_number_or_tag(BlockNumberOrTag::Number(block))
            .await
    }

    pub async fn get_logs(&self, filter: Filter) -> Result<Vec<Log>, Error> {
        self.provider
            .get_logs(&filter)
            .await
            .map_err(|e| Error::msg(format!("Failed to get logs: {e}")))
    }
}
