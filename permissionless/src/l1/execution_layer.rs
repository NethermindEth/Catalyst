#![allow(unused)] // TODO: remove this once we have a used inner, provider, and config fields

use super::bindings;
use alloy::{
    primitives::Address,
    providers::{DynProvider, Provider},
    rpc::types::{Filter, Log},
    sol_types::SolEvent,
};
use anyhow::Error;
use common::l1::{
    config::ProtocolConfig, execution_layer_inner::ExecutionLayerInner, extension::ELExtension,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct L1ContractAddresses {
    pub registry_address: Address,
}

#[derive(Clone)]
pub struct EthereumL1Config {
    contract_addresses: L1ContractAddresses,
}

pub struct ExecutionLayer {
    inner: Arc<ExecutionLayerInner>,
    provider: DynProvider,
    config: EthereumL1Config,
}

impl ELExtension for ExecutionLayer {
    type Config = EthereumL1Config;
    async fn new(
        inner: Arc<ExecutionLayerInner>,
        provider: DynProvider,
        config: Self::Config,
    ) -> Self {
        Self {
            inner,
            provider,
            config,
        }
    }

    async fn fetch_protocol_config(&self) -> Result<ProtocolConfig, Error> {
        todo!()
    }

    async fn get_preconfer_total_bonds(&self) -> Result<alloy::primitives::U256, Error> {
        todo!()
    }
}

impl ExecutionLayer {
    async fn get_logs_for_register_method(&self) -> Result<Vec<Log>, Error> {
        let registry_address = self.config.contract_addresses.registry_address;

        let filter = Filter::new()
            .address(registry_address)
            .event_signature(bindings::IRegistry::OperatorRegistered::SIGNATURE_HASH);

        let logs = self.provider.get_logs(&filter).await?;

        Ok(logs)
    }
}
