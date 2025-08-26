#![allow(unused)] // TODO: remove this once we have a used contract_addresses field

use common::utils::config_trait::ConfigTrait;
use tracing::warn;

#[derive(Debug, Clone)]
pub struct L1ContractAddresses {
    pub registry_address: String,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub contract_addresses: L1ContractAddresses,
}

impl ConfigTrait for Config {
    fn read_env_variables() -> Self {
        let default_empty_address = "0x0000000000000000000000000000000000000000".to_string();

        const REGISTRY_ADDRESS: &str = "REGISTRY_ADDRESS";
        let registry_address = std::env::var(REGISTRY_ADDRESS).unwrap_or_else(|_| {
            warn!(
                "No Registry contract address found in {} env var, using default",
                REGISTRY_ADDRESS
            );
            default_empty_address.clone()
        });

        Config {
            contract_addresses: L1ContractAddresses { registry_address },
        }
    }
}
