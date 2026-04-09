use alloy::primitives::Address;
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
    pub contract_addresses: ContractAddresses,
}
