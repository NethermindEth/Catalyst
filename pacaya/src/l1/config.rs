use crate::utils::config::{L1ContractAddresses, PacayaConfig};
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

impl TryFrom<L1ContractAddresses> for ContractAddresses {
    type Error = anyhow::Error;

    fn try_from(l1_contract_addresses: L1ContractAddresses) -> Result<Self, Self::Error> {
        Ok(ContractAddresses {
            taiko_inbox: l1_contract_addresses.taiko_inbox,
            taiko_token: OnceCell::new(),
            preconf_whitelist: l1_contract_addresses.preconf_whitelist,
            preconf_router: l1_contract_addresses.preconf_router,
            taiko_wrapper: l1_contract_addresses.taiko_wrapper,
            forced_inclusion_store: l1_contract_addresses.forced_inclusion_store,
        })
    }
}

pub struct EthereumL1Config {
    pub contract_addresses: ContractAddresses,
}

impl TryFrom<PacayaConfig> for EthereumL1Config {
    type Error = anyhow::Error;

    fn try_from(config: PacayaConfig) -> Result<Self, Self::Error> {
        Ok(EthereumL1Config {
            contract_addresses: ContractAddresses::try_from(config.contract_addresses)?,
        })
    }
}
