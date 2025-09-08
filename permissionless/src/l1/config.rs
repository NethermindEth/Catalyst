use crate::utils::config::{Config as utils_config, L1ContractAddresses};
use alloy::primitives::Address;

#[derive(Clone)]
pub struct ContractAddresses {
    pub registry_address: Address,
}

impl TryFrom<L1ContractAddresses> for ContractAddresses {
    type Error = anyhow::Error;

    fn try_from(l1_contract_addresses: L1ContractAddresses) -> Result<Self, Self::Error> {
        let registry_address = l1_contract_addresses.registry_address.parse()?;

        Ok(ContractAddresses { registry_address })
    }
}

#[derive(Clone)]
pub struct EthereumL1Config {
    pub contract_addresses: ContractAddresses,
}

impl TryFrom<utils_config> for EthereumL1Config {
    type Error = anyhow::Error;

    fn try_from(config: utils_config) -> Result<Self, Self::Error> {
        Ok(EthereumL1Config {
            contract_addresses: ContractAddresses::try_from(config.contract_addresses)?,
        })
    }
}
