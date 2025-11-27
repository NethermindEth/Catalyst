use crate::{
    config::Config,
    shared::l2_tx_lists::{self, PreBuiltTxList},
    utils::rpc_client::JSONRPCClient,
};
use alloy::primitives::Address;
use anyhow::Error;
use serde_json::Value;
use std::cmp::{max, min};
use std::time::Duration;
use tracing::debug;

pub struct L2Engine {
    auth_rpc: JSONRPCClient,
    config: L2EngineConfig,
}

pub struct L2EngineConfig {
    pub auth_url: String,
    pub rpc_timeout: Duration,
    pub jwt_secret_bytes: [u8; 32],
    pub max_bytes_per_tx_list: u64,
    pub throttling_factor: u64,
    pub min_bytes_per_tx_list: u64,
    pub coinbase: Address,
}

impl L2EngineConfig {
    pub fn new(config: &Config, coinbase: Address) -> Result<Self, Error> {
        let jwt_secret_bytes =
            crate::utils::file_operations::read_jwt_secret(&config.jwt_secret_file_path)
                .map_err(|e| anyhow::anyhow!("Failed to read JWT secret for L2 engine: {}", e))?;
        Ok(Self {
            auth_url: config.taiko_geth_auth_rpc_url.clone(),
            rpc_timeout: config.rpc_l2_execution_layer_timeout,
            jwt_secret_bytes,
            max_bytes_per_tx_list: config.max_bytes_per_tx_list,
            min_bytes_per_tx_list: config.min_bytes_per_tx_list,
            throttling_factor: config.throttling_factor,
            coinbase,
        })
    }
}

impl L2Engine {
    pub fn new(config: L2EngineConfig) -> Result<Self, Error> {
        let auth_rpc = JSONRPCClient::new_with_timeout_and_jwt(
            &config.auth_url,
            config.rpc_timeout,
            &config.jwt_secret_bytes,
        )
        .map_err(|e| {
            anyhow::anyhow!("Failed to create JSONRPCClient for taiko geth auth: {}", e)
        })?;

        Ok(Self { auth_rpc, config })
    }

    pub async fn get_pending_l2_tx_list(
        &self,
        base_fee: u64,
        batches_ready_to_send: u64,
        block_max_gas_limit: u64,
    ) -> Result<Option<PreBuiltTxList>, Error> {
        let max_bytes_per_tx_list = calculate_max_bytes_per_tx_list(
            self.config.max_bytes_per_tx_list,
            self.config.throttling_factor,
            batches_ready_to_send,
            self.config.min_bytes_per_tx_list,
        );
        let params = vec![
            Value::String(format!("0x{}", hex::encode(self.config.coinbase))), // beneficiary address
            Value::from(base_fee),                                             // baseFee
            Value::Number(block_max_gas_limit.into()),                         // blockMaxGasLimit
            Value::Number(max_bytes_per_tx_list.into()), // maxBytesPerTxList (128KB by default)
            Value::Array(vec![]),                        // locals (empty array)
            Value::Number(1.into()),                     // maxTransactionsLists
            Value::Number(0.into()),                     // minTip
        ];

        let result = self
            .auth_rpc
            .call_method("taikoAuth_txPoolContentWithMinTip", params)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get L2 tx lists: {}", e))?;
        if result != Value::Null {
            let tx_lists = l2_tx_lists::decompose_pending_lists_json_from_geth(result)
                .map_err(|e| anyhow::anyhow!("Failed to decompose L2 tx lists: {}", e))?;

            // ignoring rest of tx lists, only one list per L2 block is processed
            let first = tx_lists.into_iter().next();
            match first {
                Some(list) => Ok(Some(list)),
                _ => Ok(None),
            }
        } else {
            Ok(None)
        }
    }
}

/// Calculate the max bytes per tx list based on the number of batches ready to send.
/// The max bytes per tx list is reduced exponentially by given factor.
fn calculate_max_bytes_per_tx_list(
    max_bytes_per_tx_list: u64,
    throttling_factor: u64,
    batches_ready_to_send: u64,
    min_bytes_per_tx_list: u64,
) -> u64 {
    let mut size = max_bytes_per_tx_list;
    for _ in 0..batches_ready_to_send {
        size = size.saturating_sub(size / throttling_factor);
    }
    size = min(max_bytes_per_tx_list, max(size, min_bytes_per_tx_list));
    if batches_ready_to_send > 0 {
        debug!("Reducing max bytes per tx list to {}", size);
    }
    size
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_calculate_max_bytes_per_tx_list() {
        let max_bytes = 1000; // 128KB
        let throttling_factor = 10;
        let min_value = 100;

        // Test with no throttling (attempt = 0)
        assert_eq!(
            calculate_max_bytes_per_tx_list(max_bytes, throttling_factor, 0, min_value),
            max_bytes
        );

        assert_eq!(
            calculate_max_bytes_per_tx_list(max_bytes, throttling_factor, 1, min_value),
            900
        );

        assert_eq!(
            calculate_max_bytes_per_tx_list(max_bytes, throttling_factor, 2, min_value),
            810
        );

        assert_eq!(
            calculate_max_bytes_per_tx_list(max_bytes, throttling_factor, 3, min_value),
            729
        );

        // Test with throttling factor greater than max_bytes
        assert_eq!(calculate_max_bytes_per_tx_list(100, 200, 1, min_value), 100);

        // Test with zero max_bytes
        assert_eq!(
            calculate_max_bytes_per_tx_list(0, throttling_factor, 1, min_value),
            0
        );

        // Test with min_value
        assert_eq!(
            calculate_max_bytes_per_tx_list(max_bytes, throttling_factor, 500, min_value),
            min_value
        );
    }
}
