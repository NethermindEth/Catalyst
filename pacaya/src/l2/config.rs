use alloy::primitives::Address;
use anyhow::Error;
use common::config::Config;
use common::signer::{Signer, create_signer};
use common::utils::file_operations::read_jwt_secret;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct TaikoConfig {
    pub taiko_geth_url: String,
    pub taiko_geth_auth_url: String,
    pub driver_url: String,
    pub jwt_secret_bytes: [u8; 32],
    pub taiko_anchor_address: Address,
    pub taiko_bridge_address: Address,
    pub max_bytes_per_tx_list: u64,
    pub min_bytes_per_tx_list: u64,
    pub throttling_factor: u64,
    pub rpc_l2_execution_layer_timeout: Duration,
    pub rpc_driver_preconf_timeout: Duration,
    pub rpc_driver_status_timeout: Duration,
    pub preconf_heartbeat_ms: u64,
    pub signer: Arc<Signer>,
}

impl TaikoConfig {
    pub async fn new(config: &Config) -> Result<Self, Error> {
        let jwt_secret_bytes = read_jwt_secret(&config.jwt_secret_file_path)?;
        let signer = create_signer(
            config.web3signer_l2_url.clone(),
            config.catalyst_node_ecdsa_private_key.clone(),
            config.preconfer_address.clone(),
        )
        .await?;

        Ok(Self {
            taiko_geth_url: config.taiko_geth_rpc_url.clone(),
            taiko_geth_auth_url: config.taiko_geth_auth_rpc_url.clone(),
            driver_url: config.taiko_driver_url.clone(),
            jwt_secret_bytes,
            taiko_anchor_address: Address::from_str(&config.taiko_anchor_address)?,
            taiko_bridge_address: Address::from_str(&config.taiko_bridge_address)?,
            max_bytes_per_tx_list: config.max_bytes_per_tx_list,
            min_bytes_per_tx_list: config.min_bytes_per_tx_list,
            throttling_factor: config.throttling_factor,
            rpc_l2_execution_layer_timeout: config.rpc_l2_execution_layer_timeout,
            rpc_driver_preconf_timeout: config.rpc_driver_preconf_timeout,
            rpc_driver_status_timeout: config.rpc_driver_status_timeout,
            preconf_heartbeat_ms: config.preconf_heartbeat_ms,
            signer,
        })
    }
}
