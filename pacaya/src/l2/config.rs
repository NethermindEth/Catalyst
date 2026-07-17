use alloy::primitives::Address;
use anyhow::Error;
use common::config::Config;
use common::signer::{Signer, Web3SignerInfo, create_signer};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct TaikoConfig {
    pub l2_rpc_url: String,
    pub driver_url: String,
    pub jwt_secret_bytes: [u8; 32],
    pub anchor_address: Address,
    pub bridge_l2_address: Address,
    pub rpc_driver_preconf_timeout: Duration,
    pub rpc_driver_status_timeout: Duration,
    pub rpc_driver_retry_timeout: Duration,
    pub preconf_heartbeat_ms: u64,
    pub signer: Arc<Signer>,
}

impl TaikoConfig {
    pub async fn new(config: &Config) -> Result<Self, Error> {
        let jwt_secret_bytes =
            common::utils::file_operations::read_jwt_secret(&config.jwt_secret_file_path)
                .map_err(|e| anyhow::anyhow!("Failed to read JWT secret for Taiko: {}", e))?;
        let w3s_info = Web3SignerInfo::new(
            config.web3signer_l2_url.clone(),
            config.web3signer_root_certificate_path.clone(),
            config.web3signer_client_certificate_path.clone(),
            config.web3signer_client_key_path.clone(),
            config.preconfer_address,
        )?;
        let signer =
            create_signer(w3s_info, config.catalyst_node_ecdsa_private_key.clone()).await?;

        Ok(Self {
            l2_rpc_url: config.l2_rpc_url.clone(),
            driver_url: config.l2_driver_url.clone(),
            jwt_secret_bytes,
            anchor_address: config.anchor_address,
            bridge_l2_address: config.bridge_l2_address,
            rpc_driver_preconf_timeout: config.rpc_driver_preconf_timeout,
            rpc_driver_status_timeout: config.rpc_driver_status_timeout,
            rpc_driver_retry_timeout: config.rpc_driver_retry_timeout,
            preconf_heartbeat_ms: config.preconf_heartbeat_ms,
            signer,
        })
    }
}
