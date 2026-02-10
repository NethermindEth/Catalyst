use alloy::primitives::U256;
use anyhow::Error;
use common::utils::rpc_client::JSONRPCClient;
use std::time::Duration;
use taiko_preconfirmation_driver::rpc::{PreconfSlotInfo, server::METHOD_GET_PRECONF_SLOT_INFO};
use tracing::debug;

/// Client for communicating with the preconfirmation driver's JSON-RPC server.
///
/// Provides a typed wrapper around the `preconf_getPreconfSlotInfo` RPC method
/// exposed by the preconfirmation driver node.
pub struct PreconfirmationDriver {
    rpc_client: JSONRPCClient,
}

impl PreconfirmationDriver {
    pub fn new_with_timeout(url: &str, timeout: Duration) -> Result<Self, Error> {
        let rpc_client = JSONRPCClient::new_with_timeout(url, timeout)?;
        Ok(Self { rpc_client })
    }

    pub async fn get_preconf_slot_info(&self, timestamp: U256) -> Result<PreconfSlotInfo, Error> {
        debug!("Calling {}", METHOD_GET_PRECONF_SLOT_INFO);
        let response = self
            .rpc_client
            .call_method(
                METHOD_GET_PRECONF_SLOT_INFO,
                vec![serde_json::to_value(timestamp)?],
            )
            .await?;
        let slot_info: PreconfSlotInfo = serde_json::from_value(response)?;
        Ok(slot_info)
    }
}
