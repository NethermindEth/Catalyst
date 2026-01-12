use crate::config::Config;
use crate::signer::{Signer, create_signer};
use alloy::primitives::Address;
use anyhow::Error;
use std::sync::Arc;

#[derive(Clone)]
pub struct EthereumL1Config {
    pub execution_rpc_urls: Vec<String>,
    pub consensus_rpc_url: String,
    pub blob_indexer_url: Option<String>,
    pub min_priority_fee_per_gas_wei: u64,
    pub tx_fees_increase_percentage: u64,
    pub slot_duration_sec: u64,
    pub slots_per_epoch: u64,
    pub preconf_heartbeat_ms: u64,
    pub max_attempts_to_send_tx: u64,
    pub max_attempts_to_wait_tx: u64,
    pub delay_between_tx_attempts_sec: u64,
    pub signer: Arc<Signer>,
    pub preconfer_address: Option<Address>,
    pub extra_gas_percentage: u64,
}

impl EthereumL1Config {
    pub async fn new(config: &Config) -> Result<Self, Error> {
        let signer = create_signer(
            config.web3signer_l1_url.clone(),
            config.catalyst_node_ecdsa_private_key.clone(),
            config.preconfer_address,
        )
        .await?;

        Ok(Self {
            execution_rpc_urls: config.l1_rpc_urls.clone(),
            consensus_rpc_url: config.l1_beacon_url.clone(),
            blob_indexer_url: config.blob_indexer_url.clone(),
            slot_duration_sec: config.l1_slot_duration_sec,
            slots_per_epoch: config.l1_slots_per_epoch,
            preconf_heartbeat_ms: config.preconf_heartbeat_ms,
            min_priority_fee_per_gas_wei: config.min_priority_fee_per_gas_wei,
            tx_fees_increase_percentage: config.tx_fees_increase_percentage,
            max_attempts_to_send_tx: config.max_attempts_to_send_tx,
            max_attempts_to_wait_tx: config.max_attempts_to_wait_tx,
            delay_between_tx_attempts_sec: config.delay_between_tx_attempts_sec,
            signer,
            preconfer_address: config.preconfer_address,
            extra_gas_percentage: config.extra_gas_percentage,
        })
    }
}
