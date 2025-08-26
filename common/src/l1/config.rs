use crate::shared::signer::Signer;
use alloy::primitives::Address;
use std::sync::Arc;
use tokio::sync::OnceCell;

#[derive(Clone)]
pub struct ContractAddresses {
    pub taiko_inbox: Address,
    pub taiko_token: OnceCell<Address>,
    pub preconf_whitelist: Address,
    pub preconf_router: Address,
    pub taiko_wrapper: Address,
    pub forced_inclusion_store: Address,
}

pub struct EthereumL1Config {
    pub execution_rpc_urls: Vec<String>,
    pub contract_addresses: ContractAddresses,
    pub consensus_rpc_url: String,
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
