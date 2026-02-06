use crate::l1::bindings::PreconfWhitelist;
use alloy::{
    primitives::Address,
    providers::{DynProvider, Provider},
    rpc::client::BatchRequest,
    sol_types::SolCall,
};
use anyhow::Error;
use serde_json::json;
use std::sync::{OnceLock, RwLock};

pub enum OperatorError {
    OperatorCheckTooEarly,
    Any(Error),
}

/// Cached result of get_operators_for_current_and_next_epoch.
/// Operators only change once per L1 slot (12s), so we avoid repeating the RPC call every L2 slot (2s).
/// Key is current_slot_timestamp.
pub struct OperatorsCache {
    cache: RwLock<Option<(u64, (Address, Address))>>,
    provider: DynProvider,
    whitelist_address: Address,
}

impl OperatorsCache {
    pub fn new(provider: DynProvider, whitelist_address: Address) -> Self {
        Self {
            cache: RwLock::new(None),
            provider,
            whitelist_address,
        }
    }

    pub async fn get_operators_for_current_and_next_epoch(
        &self,
        current_epoch_timestamp: u64,
        current_slot_timestamp: u64,
    ) -> Result<(Address, Address), OperatorError> {
        if let Ok(guard) = self.cache.read()
            && let Some((cached_ts, addresses)) = *guard
            && cached_ts == current_slot_timestamp
        {
            return Ok(addresses);
        }
        let result = self
            .get_operators_for_current_and_next_epoch_internal(current_epoch_timestamp)
            .await?;
        if let Ok(mut guard) = self.cache.write() {
            *guard = Some((current_slot_timestamp, result));
        }
        Ok(result)
    }

    async fn get_operators_for_current_and_next_epoch_internal(
        &self,
        current_epoch_timestamp: u64,
    ) -> Result<(Address, Address), OperatorError> {
        tracing::debug!(
            "get_operators_for_current_and_next_epoch_internal, for timestamp: {}",
            current_epoch_timestamp
        );
        let contract = PreconfWhitelist::new(self.whitelist_address, &self.provider);
        let current_epoch_call_data = Self::get_current_epoch_call_data(&contract);
        let next_epoch_call_data = Self::get_next_epoch_call_data(&contract);

        // Use BatchRequest to send all calls in a single RPC request
        // This ensures the load balancer forwards all calls to the same RPC node
        let client = self.provider.client();
        let mut batch = BatchRequest::new(client);

        let block_waiter = batch
            .add_call("eth_getBlockByNumber", &("latest", false))
            .map_err(|e| {
                OperatorError::Any(Error::msg(format!(
                    "Failed to add block call to batch: {e}"
                )))
            })?;

        let current_operator_call_params = json!([{
        "to": self.whitelist_address,
        "data": format!("0x{}", hex::encode(current_epoch_call_data))
    }, "latest"]);
        let current_operator_waiter = batch
            .add_call("eth_call", &current_operator_call_params)
            .map_err(|e| {
                OperatorError::Any(Error::msg(format!(
                    "Failed to add current operator call to batch: {e}"
                )))
            })?;

        let next_operator_call_params = json!([{
        "to": self.whitelist_address,
        "data": format!("0x{}", hex::encode(next_epoch_call_data))
    }, "latest"]);
        let next_operator_waiter = batch
            .add_call("eth_call", &next_operator_call_params)
            .map_err(|e| {
                OperatorError::Any(Error::msg(format!(
                    "Failed to add next operator call to batch: {e}"
                )))
            })?;

        batch.send().await.map_err(|e| {
            OperatorError::Any(Error::msg(format!("Failed to send batch request: {e}")))
        })?;

        let block_result: serde_json::Value = block_waiter.await.map_err(|e| {
            OperatorError::Any(Error::msg(format!("Failed to get block from batch: {e}")))
        })?;
        let block: alloy::rpc::types::Block = serde_json::from_value(block_result)
            .map_err(|e| OperatorError::Any(Error::msg(format!("Failed to parse block: {e}"))))?;
        let latest_block_timestamp = block.header.timestamp;
        if latest_block_timestamp < current_epoch_timestamp {
            return Err(OperatorError::OperatorCheckTooEarly);
        }

        let current_operator_result: serde_json::Value =
            current_operator_waiter.await.map_err(|e| {
                OperatorError::Any(Error::msg(format!(
                    "Failed to get current operator from batch: {}, contract: {:?}",
                    e, self.whitelist_address
                )))
            })?;

        let next_operator_result: serde_json::Value = next_operator_waiter.await.map_err(|e| {
            OperatorError::Any(Error::msg(format!(
                "Failed to get next operator from batch: {}, contract: {:?}",
                e, self.whitelist_address
            )))
        })?;

        let current_operator_bytes = hex::decode(
            current_operator_result
                .as_str()
                .ok_or_else(|| {
                    OperatorError::Any(Error::msg("Invalid current operator result format"))
                })?
                .strip_prefix("0x")
                .unwrap_or_default(),
        )
        .map_err(|e| {
            OperatorError::Any(Error::msg(format!(
                "Failed to decode current operator: {e}"
            )))
        })?;
        let current_operator =
            <PreconfWhitelist::getOperatorForCurrentEpochCall as SolCall>::abi_decode_returns(
                &current_operator_bytes,
            )
            .map_err(|e| {
                OperatorError::Any(Error::msg(format!(
                    "Failed to decode current operator response: {e}"
                )))
            })?;

        let next_operator_bytes = hex::decode(
            next_operator_result
                .as_str()
                .ok_or_else(|| {
                    OperatorError::Any(Error::msg("Invalid next operator result format"))
                })?
                .strip_prefix("0x")
                .unwrap_or_default(),
        )
        .map_err(|e| {
            OperatorError::Any(Error::msg(format!("Failed to decode next operator: {e}")))
        })?;
        let next_operator =
            <PreconfWhitelist::getOperatorForNextEpochCall as SolCall>::abi_decode_returns(
                &next_operator_bytes,
            )
            .map_err(|e| {
                OperatorError::Any(Error::msg(format!(
                    "Failed to decode next operator response: {e}"
                )))
            })?;

        Ok((current_operator, next_operator))
    }

    /// cached as constant since function has no parameters
    fn get_current_epoch_call_data(
        contract: &PreconfWhitelist::PreconfWhitelistInstance<&DynProvider>,
    ) -> &'static [u8] {
        static CALL_DATA: OnceLock<Vec<u8>> = OnceLock::new();
        CALL_DATA.get_or_init(|| {
        let tx_req = contract
            .getOperatorForCurrentEpoch()
            .into_transaction_request();
        tx_req
            .input
            .input
            .as_ref()
            .expect("get_current_epoch_call_data: Failed to get current epoch call data. Check the whitelist contract bindings.")
            .to_vec()
    })
    }

    /// cached as constant since function has no parameters
    fn get_next_epoch_call_data(
        contract: &PreconfWhitelist::PreconfWhitelistInstance<&DynProvider>,
    ) -> &'static [u8] {
        static CALL_DATA: OnceLock<Vec<u8>> = OnceLock::new();
        CALL_DATA.get_or_init(|| {
        let tx_req = contract
            .getOperatorForNextEpoch()
            .into_transaction_request();
        tx_req.input.input.as_ref().expect("get_next_epoch_call_data: Failed to get next epoch call data. Check the whitelist contract bindings.").to_vec()
    })
    }
}
