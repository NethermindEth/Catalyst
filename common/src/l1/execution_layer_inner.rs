use crate::{
    l1::{monitor_transaction::TransactionMonitor, transaction_error::TransactionError},
    metrics,
};
use alloy::{
    primitives::Address,
    providers::{DynProvider, Provider},
};
use anyhow::Error;
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
        provider: DynProvider,
        preconfer_address: Address,
        config_common: super::config::EthereumL1Config,
        transaction_error_channel: Sender<TransactionError>,
        metrics: Arc<metrics::Metrics>,
    ) -> Result<Self, Error> {
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
}
