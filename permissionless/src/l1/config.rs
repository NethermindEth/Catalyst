#![allow(dead_code)]

use crate::utils::config::{Config as utils_config, L1ContractAddresses};
use alloy::primitives::Address;

#[derive(Clone)]
pub struct ContractAddresses {
    pub registry_address: Address,
    pub lookahead_store_address: Address,
    pub lookahead_slasher_address: Address,
    pub preconf_slasher_address: Address,
}

impl TryFrom<L1ContractAddresses> for ContractAddresses {
    type Error = anyhow::Error;

    fn try_from(l1_contract_addresses: L1ContractAddresses) -> Result<Self, Self::Error> {
        Ok(ContractAddresses {
            registry_address: l1_contract_addresses.registry_address,
            lookahead_store_address: l1_contract_addresses.lookahead_store_address,
            lookahead_slasher_address: l1_contract_addresses.lookahead_slasher_address,
            preconf_slasher_address: l1_contract_addresses.preconf_slasher_address,
        })
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
