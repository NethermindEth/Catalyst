#![allow(unused)] // TODO: remove this once we have a used inner, provider, and config fields

use super::config::EthereumL1Config;
use alloy::{
    primitives::Address,
    providers::{DynProvider, Provider},
    rpc::types::{Filter, Log},
    sol_types::SolEvent,
};
use anyhow::{Error, anyhow};
use common::{
    l1::{traits::ELTrait, transaction_error::TransactionError},
    metrics::Metrics,
    shared::alloy_tools,
    shared::execution_layer::ExecutionLayer as ExecutionLayerCommon,
};
use pacaya::l1::protocol_config::{BaseFeeConfig, ProtocolConfig};
use std::sync::Arc;
use tokio::sync::mpsc::Sender;

pub struct ExecutionLayer {
    common: ExecutionLayerCommon,
    provider: DynProvider,
    config: EthereumL1Config,
}

impl ELTrait for ExecutionLayer {
    type Config = EthereumL1Config;
    async fn new(
        common_config: common::l1::config::EthereumL1Config,
        specific_config: Self::Config,
        transaction_error_channel: Sender<TransactionError>,
        metrics: Arc<Metrics>,
    ) -> Result<Self, Error> {
        let provider = alloy_tools::construct_alloy_provider(
            &common_config.signer,
            common_config
                .execution_rpc_urls
                .first()
                .ok_or_else(|| anyhow!("L1 RPC URL is required"))?,
        )
        .await?;
        let protocol_config = ProtocolConfig::default();

        let common = ExecutionLayerCommon::new(provider.clone()).await?;

        Ok(Self {
            common,
            provider,
            config: specific_config,
        })
    }

    fn common(&self) -> &ExecutionLayerCommon {
        &self.common
    }
}

impl ExecutionLayer {
    async fn get_logs_for_register_method(&self) -> Result<Vec<Log>, Error> {
        let registry_address = self.config.contract_addresses.registry_address;

        let filter = Filter::new()
            .address(registry_address)
            .event_signature(urc::bindings::IRegistry::OperatorRegistered::SIGNATURE_HASH);

        let logs = self.provider.get_logs(&filter).await?;

        Ok(logs)
    }
}
