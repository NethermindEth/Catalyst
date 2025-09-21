#![allow(unused)] // TODO: remove this once we have a used contract_addresses field

use common::utils::config_trait::ConfigTrait;
use std::fmt;
use tracing::warn;

#[derive(Debug, Clone)]
pub struct L1ContractAddresses {
    pub registry_address: String,
    pub lookahead_store_address: String,
    pub lookahead_slasher_address: String,
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

        const LOOKAHEAD_STORE_ADDRESS: &str = "LOOKAHEAD_STORE_ADDRESS";
        let lookahead_store_address = std::env::var(LOOKAHEAD_STORE_ADDRESS).unwrap_or_else(|_| {
            warn!(
                "No Lookahead Store contract address found in {} env var, using default",
                LOOKAHEAD_STORE_ADDRESS
            );
            default_empty_address.clone()
        });

        const LOOKAHEAD_SLASHER_ADDRESS: &str = "LOOKAHEAD_SLASHER_ADDRESS";
        let lookahead_slasher_address =
            std::env::var(LOOKAHEAD_SLASHER_ADDRESS).unwrap_or_else(|_| {
                warn!(
                    "No Lookahead Slasher contract address found in {} env var, using default",
                    LOOKAHEAD_SLASHER_ADDRESS
                );
                default_empty_address.clone()
            });

        Config {
            contract_addresses: L1ContractAddresses {
                registry_address,
                lookahead_store_address,
                lookahead_slasher_address,
            },
        }
    }
}

impl fmt::Display for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Contract addresses: {:#?}", self.contract_addresses)?;

        Ok(())
    }
}
