#![allow(unused)] // TODO: remove this once we have a used contract_addresses field

use alloy::hex;
use alloy::primitives::Address;
use anyhow::Error;
use common::config::{ConfigTrait, address_parse_error};
use secp256k1::SecretKey;
use std::{fmt, str::FromStr, time::Duration};
use tracing::warn;

#[derive(Debug, Clone)]
pub struct L1ContractAddresses {
    pub registry_address: Address,
    pub lookahead_store_address: Address,
    pub lookahead_slasher_address: Address,
    pub preconf_slasher_address: Address,
}

#[derive(Clone)]
pub struct Config {
    pub contract_addresses: L1ContractAddresses,
    pub preconfirmation_driver_url: String,
    pub preconfirmation_driver_timeout: Duration,
    pub shasta_inbox: Address,
    pub l1_height_lag: u64,
    pub max_blocks_to_reanchor: u64,
    pub propose_forced_inclusion: bool,
    pub sequencer_key: SecretKey,
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

        const PRECONFIRMATION_DRIVER_URL: &str = "PRECONFIRMATION_DRIVER_URL";
        let preconfirmation_driver_url = std::env::var(PRECONFIRMATION_DRIVER_URL)
            .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", PRECONFIRMATION_DRIVER_URL, e))?;
        const PRECONFIRMATION_DRIVER_TIMEOUT_MS: &str = "PRECONFIRMATION_DRIVER_TIMEOUT_MS";
        let preconfirmation_driver_timeout = Duration::from_millis(
            std::env::var(PRECONFIRMATION_DRIVER_TIMEOUT_MS)
                .unwrap_or("1500".to_string())
                .parse::<u64>()
                .map_err(|e| {
                    anyhow::anyhow!(
                        "Failed to read {}: {}",
                        PRECONFIRMATION_DRIVER_TIMEOUT_MS,
                        e
                    )
                })?,
        );

        const SHASTA_INBOX_ADDRESS: &str = "SHASTA_INBOX_ADDRESS";
        let shasta_inbox_str = std::env::var(SHASTA_INBOX_ADDRESS)
            .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", SHASTA_INBOX_ADDRESS, e))?;
        let shasta_inbox = Address::from_str(&shasta_inbox_str)
            .map_err(|e| address_parse_error(SHASTA_INBOX_ADDRESS, e, &shasta_inbox_str))?;

        let l1_height_lag = std::env::var("L1_HEIGHT_LAG")
            .unwrap_or("4".to_string())
            .parse::<u64>()
            .map_err(|e| anyhow::anyhow!("L1_HEIGHT_LAG must be a number: {}", e))?;

        let max_blocks_to_reanchor = std::env::var("MAX_BLOCKS_TO_REANCHOR")
            .unwrap_or("1000".to_string())
            .parse::<u64>()
            .map_err(|e| anyhow::anyhow!("MAX_BLOCKS_TO_REANCHOR must be a number: {}", e))?;

        let propose_forced_inclusion = std::env::var("PROPOSE_FORCED_INCLUSION")
            .unwrap_or("true".to_string())
            .parse::<bool>()
            .map_err(|e| anyhow::anyhow!("PROPOSE_FORCED_INCLUSION must be a boolean: {}", e))?;

        let sequencer_key_str = std::env::var("SEQUENCER_KEY")
            .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", "SEQUENCER_KEY", e))?;
        let sequencer_key_bytes = hex::decode(sequencer_key_str.trim_start_matches("0x"))
            .map_err(|e| anyhow::anyhow!("{} must be valid hex: {}", "SEQUENCER_KEY", e))?;
        let sequencer_key = SecretKey::from_slice(&sequencer_key_bytes).map_err(|e| {
            anyhow::anyhow!("{} must be a valid secp256k1 key: {}", "SEQUENCER_KEY", e)
        })?;

        Ok(Config {
            contract_addresses: L1ContractAddresses {
                registry_address,
                lookahead_store_address,
                lookahead_slasher_address,
                preconf_slasher_address,
            },
            preconfirmation_driver_url,
            preconfirmation_driver_timeout,
            shasta_inbox,
            l1_height_lag,
            max_blocks_to_reanchor,
            propose_forced_inclusion,
            sequencer_key,
        })
    }
}

impl fmt::Display for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Contract addresses: {:#?}", self.contract_addresses)?;

        Ok(())
    }
}
