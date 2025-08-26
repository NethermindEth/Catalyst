use super::bindings::forced_inclusion_store::{
    IForcedInclusionStore, IForcedInclusionStore::ForcedInclusion,
};
use alloy::{
    primitives::{Address, U256},
    providers::DynProvider,
};
use anyhow::{Error, anyhow};
use common::l1::{
    bindings::BatchParams, config::ContractAddresses, execution_layer_inner::ExecutionLayerInner,
    extension::ELExtension, forced_inclusion_info::ForcedInclusionInfo,
    propose_batch_builder::ProposeBatchBuilder,
};
use std::sync::Arc;

pub struct EthereumL1Config {
    pub contract_addresses: ContractAddresses,
}

pub struct ExecutionLayer {
    inner: Arc<ExecutionLayerInner>,
    provider: DynProvider,
    config: EthereumL1Config,
}

impl ELExtension for ExecutionLayer {
    type Config = EthereumL1Config;
    fn new(
        inner: Arc<ExecutionLayerInner>,
        provider: DynProvider,
        config: EthereumL1Config,
    ) -> Self {
        Self {
            inner,
            provider,
            config,
        }
    }
}

impl ExecutionLayer {
    pub async fn get_forced_inclusion_head(&self) -> Result<u64, Error> {
        let contract = IForcedInclusionStore::new(
            self.config.contract_addresses.forced_inclusion_store,
            self.provider.clone(),
        );
        contract
            .head()
            .call()
            .await
            .map_err(|e| anyhow!("Failed to get forced inclusion head: {}", e))
    }

    pub async fn get_forced_inclusion_tail(&self) -> Result<u64, Error> {
        let contract = IForcedInclusionStore::new(
            self.config.contract_addresses.forced_inclusion_store,
            self.provider.clone(),
        );
        contract
            .tail()
            .call()
            .await
            .map_err(|e| anyhow!("Failed to get forced inclusion tail: {}", e))
    }

    pub async fn get_forced_inclusion(&self, index: u64) -> Result<ForcedInclusion, Error> {
        let contract = IForcedInclusionStore::new(
            self.config.contract_addresses.forced_inclusion_store,
            self.provider.clone(),
        );
        contract
            .getForcedInclusion(U256::from(index))
            .call()
            .await
            .map_err(|e| {
                Error::msg(format!(
                    "Failed to get forced inclusion at index {index}: {e}"
                ))
            })
    }

    pub fn build_forced_inclusion_batch(
        &self,
        coinbase: Address,
        last_anchor_origin_height: u64,
        last_l2_block_timestamp: u64,
        info: &ForcedInclusionInfo,
    ) -> BatchParams {
        ProposeBatchBuilder::build_forced_inclusion_batch(
            self.inner.preconfer_address(),
            coinbase,
            last_anchor_origin_height,
            last_l2_block_timestamp,
            info,
        )
    }
}
