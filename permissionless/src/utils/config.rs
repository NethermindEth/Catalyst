#![allow(unused)] // TODO: remove this once we have a used contract_addresses field

use alloy::primitives::Address;
use anyhow::Error;
use common::config::{ConfigTrait, address_parse_error};
use std::fmt;
use std::str::FromStr;
use tracing::warn;

#[derive(Debug, Clone)]
pub struct L1ContractAddresses {
    pub registry_address: Address,
    pub lookahead_store_address: Address,
    pub lookahead_slasher_address: Address,
    pub preconf_slasher_address: Address,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub contract_addresses: L1ContractAddresses,
}

impl ConfigTrait for Config {
    fn read_env_variables() -> Result<Self, Error> {
        const REGISTRY_ADDRESS: &str = "REGISTRY_ADDRESS";
        let registry_address_str = std::env::var(REGISTRY_ADDRESS)
            .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", REGISTRY_ADDRESS, e))?;
        let registry_address = Address::from_str(&registry_address_str)
            .map_err(|e| address_parse_error(REGISTRY_ADDRESS, e, &registry_address_str))?;

        const LOOKAHEAD_STORE_ADDRESS: &str = "LOOKAHEAD_STORE_ADDRESS";
        let lookahead_store_address_str = std::env::var(LOOKAHEAD_STORE_ADDRESS)
            .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", LOOKAHEAD_STORE_ADDRESS, e))?;
        let lookahead_store_address =
            Address::from_str(&lookahead_store_address_str).map_err(|e| {
                address_parse_error(LOOKAHEAD_STORE_ADDRESS, e, &lookahead_store_address_str)
            })?;

        const LOOKAHEAD_SLASHER_ADDRESS: &str = "LOOKAHEAD_SLASHER_ADDRESS";
        let lookahead_slasher_address_str = std::env::var(LOOKAHEAD_SLASHER_ADDRESS)
            .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", LOOKAHEAD_SLASHER_ADDRESS, e))?;
        let lookahead_slasher_address =
            Address::from_str(&lookahead_slasher_address_str).map_err(|e| {
                address_parse_error(LOOKAHEAD_SLASHER_ADDRESS, e, &lookahead_slasher_address_str)
            })?;

        const PRECONF_SLASHER_ADDRESS: &str = "PRECONF_SLASHER_ADDRESS";
        let preconf_slasher_address_str = std::env::var(PRECONF_SLASHER_ADDRESS)
            .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", PRECONF_SLASHER_ADDRESS, e))?;
        let preconf_slasher_address =
            Address::from_str(&preconf_slasher_address_str).map_err(|e| {
                address_parse_error(PRECONF_SLASHER_ADDRESS, e, &preconf_slasher_address_str)
            })?;

        Ok(Config {
            contract_addresses: L1ContractAddresses {
                registry_address,
                lookahead_store_address,
                lookahead_slasher_address,
                preconf_slasher_address,
            },
        })
    }
}

impl fmt::Display for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Contract addresses: {:#?}", self.contract_addresses)?;

        Ok(())
    }
}
