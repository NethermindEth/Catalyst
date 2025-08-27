use alloy::{
    network::{TransactionBuilder, TransactionBuilder4844},
    providers::{DynProvider, Provider},
    rpc::types::TransactionRequest,
};
use anyhow::Error;

pub struct FeesPerGas {
    base_fee_per_gas: u128,
    base_fee_per_blob_gas: u128,
    max_fee_per_gas: u128,
    max_priority_fee_per_gas: u128,
}

impl FeesPerGas {
    pub fn update_eip1559(&self, tx: TransactionRequest, gas_limit: u64) -> TransactionRequest {
        tx.with_gas_limit(gas_limit)
            .with_max_fee_per_gas(self.max_fee_per_gas)
            .with_max_priority_fee_per_gas(self.max_priority_fee_per_gas)
    }

    pub fn update_eip4844(&self, tx: TransactionRequest, gas_limit: u64) -> TransactionRequest {
        tx.with_gas_limit(gas_limit)
            .with_max_fee_per_gas(self.max_fee_per_gas)
            .with_max_priority_fee_per_gas(self.max_priority_fee_per_gas)
            .with_max_fee_per_blob_gas(self.base_fee_per_blob_gas)
    }

    pub async fn get_eip1559_cost(&self, gas_used: u64) -> u128 {
        (self.base_fee_per_gas + self.max_priority_fee_per_gas) * u128::from(gas_used)
    }

    pub async fn get_eip4844_cost(&self, blob_count: u64, gas_used: u64) -> u128 {
        let blob_gas_used = alloy::eips::eip4844::DATA_GAS_PER_BLOB * blob_count;
        let execution_gas_cost =
            u128::from(gas_used) * (self.base_fee_per_gas + self.max_priority_fee_per_gas);
        let blob_gas_cost = u128::from(blob_gas_used) * self.base_fee_per_blob_gas;
        execution_gas_cost + blob_gas_cost
    }

    pub async fn get_fees_per_gas(provider_ws: &DynProvider) -> Result<Self, Error> {
        // Get base fee per gas
        let fee_history = provider_ws
            .get_fee_history(2, alloy::eips::BlockNumberOrTag::Latest, &[])
            .await?;

        let base_fee_per_gas = fee_history
            .base_fee_per_gas
            .last()
            .copied()
            .ok_or_else(|| anyhow::Error::msg("Failed to get base_fee_per_gas from fee history"))?;

        let base_fee_per_blob_gas = fee_history
            .base_fee_per_blob_gas
            .last()
            .copied()
            .ok_or_else(|| {
                anyhow::Error::msg("Failed to get base_fee_per_blob_gas from fee history")
            })?;

        let eip1559_estimation = provider_ws.estimate_eip1559_fees().await?;

        tracing::info!(
            ">max_fee_per_gas: {} base fee + priority fee: {}",
            eip1559_estimation.max_fee_per_gas,
            base_fee_per_gas + eip1559_estimation.max_priority_fee_per_gas
        );

        Ok(Self {
            base_fee_per_gas,
            base_fee_per_blob_gas,
            max_fee_per_gas: eip1559_estimation.max_fee_per_gas,
            max_priority_fee_per_gas: eip1559_estimation.max_priority_fee_per_gas,
        })
    }
}
