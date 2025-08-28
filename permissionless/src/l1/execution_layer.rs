#![allow(unused)] // TODO: remove this once we have a used inner, provider, and config fields

use super::bindings;
use alloy::{
    primitives::Address,
    providers::{DynProvider, Provider},
    rpc::types::{Filter, Log},
    sol_types::SolEvent,
};
use anyhow::{Error, anyhow};
use common::{
    l1::{
        config::{BaseFeeConfig, ProtocolConfig},
        el_trait::ELTrait,
        execution_layer::ExecutionLayer as ExecutionLayerCommon,
        transaction_error::TransactionError,
    },
    metrics::Metrics,
    shared::alloy_tools,
};
use std::sync::Arc;
use tokio::sync::mpsc::Sender;

#[derive(Clone)]
pub struct L1ContractAddresses {
    pub registry_address: Address,
}

#[derive(Clone)]
pub struct EthereumL1Config {
    contract_addresses: L1ContractAddresses,
}

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
        let (provider, preconfer_address) = alloy_tools::construct_alloy_provider(
            &common_config.signer,
            common_config
                .execution_rpc_urls
                .first()
                .ok_or_else(|| anyhow!("L1 RPC URL is required"))?,
            common_config.preconfer_address,
        )
        .await?;
        let protocol_config = ProtocolConfig {
            base_fee_config: BaseFeeConfig {
                adjustment_quotient: 0,
                sharing_pctg: 0,
                gas_issuance_per_second: 0,
                min_gas_excess: 0,
                max_gas_issuance_per_block: 0,
            },
            max_blocks_per_batch: 0,
            max_anchor_height_offset: 0,
            block_max_gas_limit: 0,
        };

        let common = ExecutionLayerCommon::new(
            common_config,
            transaction_error_channel,
            metrics,
            protocol_config,
            provider.clone(),
            preconfer_address,
        )
        .await?;

        Ok(Self {
            common,
            provider,
            config: specific_config,
        })
    }

    async fn get_preconfer_total_bonds(&self) -> Result<alloy::primitives::U256, Error> {
        todo!()
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
            .event_signature(bindings::IRegistry::OperatorRegistered::SIGNATURE_HASH);

        let logs = self.provider.get_logs(&filter).await?;

        Ok(logs)
    }
}
