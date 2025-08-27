use super::{
    config::{EthereumL1Config, ProtocolConfig},
    execution_layer_inner::ExecutionLayerInner,
    extension::ELExtension,
    transaction_error::TransactionError,
};
use crate::{metrics::Metrics, utils::types::*};
use alloy::primitives::Address;
use anyhow::Error;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;

pub struct ExecutionLayer<T: ELExtension> {
    pub inner: Arc<ExecutionLayerInner>,
    pub extension: Arc<T>,
    protocol_config: ProtocolConfig,
}

impl<T: ELExtension> ExecutionLayer<T> {
    pub async fn new(
        config_common: EthereumL1Config,
        specific_config: T::Config,
        transaction_error_channel: Sender<TransactionError>,
        metrics: Arc<Metrics>,
    ) -> Result<Self, Error> {
        let inner = Arc::new(
            ExecutionLayerInner::new(config_common.clone(), transaction_error_channel, metrics)
                .await?,
        );
        let extension = Arc::new(T::new(inner.clone(), inner.provider(), specific_config).await);
        let protocol_config = extension.fetch_protocol_config().await?;

        Ok(Self {
            inner,
            extension,
            protocol_config,
        })
    }

    pub fn chain_id(&self) -> u64 {
        self.inner.chain_id()
    }

    pub async fn is_transaction_in_progress(&self) -> Result<bool, Error> {
        self.inner
            .transaction_monitor
            .is_transaction_in_progress()
            .await
    }

    pub fn get_preconfer_alloy_address(&self) -> Address {
        self.inner.preconfer_address()
    }

    pub fn get_preconfer_address(&self) -> PreconferAddress {
        self.inner.preconfer_address().into_array()
    }

    pub fn get_block_max_gas_limit(&self) -> u32 {
        self.protocol_config.block_max_gas_limit
    }

    pub fn get_config_max_blocks_per_batch(&self) -> u16 {
        self.protocol_config.max_blocks_per_batch
    }

    pub fn get_config_max_anchor_height_offset(&self) -> u64 {
        self.protocol_config.max_anchor_height_offset
    }

    pub fn get_config_block_max_gas_limit(&self) -> u32 {
        self.protocol_config.block_max_gas_limit
    }

    pub fn get_protocol_config(&self) -> ProtocolConfig {
        self.protocol_config.clone()
    }
}
