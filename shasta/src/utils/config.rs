use alloy::primitives::Address;
use anyhow::Error;
use common::config::{ConfigTrait, address_parse_error};
use std::str::FromStr;

/// Temporarily allow the fields in `L1ContractAddresses` which are not read yet
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct L1ContractAddresses {
    pub preconf_whitelist: Address,
    pub preconf_router: Address,
    pub taiko_wrapper: Address,
    pub forced_inclusion_store: Address,
}
#[derive(Debug, Clone)]
pub struct ShastaConfig {
    pub shasta_inbox: Address,
    pub contract_addresses: L1ContractAddresses,
    pub handover_window_slots: u64,
    pub handover_start_buffer_ms: u64,
    pub l1_height_lag: u64,
    pub propose_forced_inclusion: bool,
    pub simulate_not_submitting_at_the_end_of_epoch: bool,
}

impl ConfigTrait for ShastaConfig {
    fn read_env_variables() -> Result<Self, Error> {
        let read_contract_address = |env_var: &str| -> Result<Address, Error> {
            let address_str = std::env::var(env_var)
                .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", env_var, e))?;
            Address::from_str(&address_str)
                .map_err(|e| address_parse_error(env_var, e, &address_str))
        };

        let shasta_inbox = read_contract_address("SHASTA_INBOX_ADDRESS")?;

        let preconf_whitelist = read_contract_address("PRECONF_WHITELIST_ADDRESS")?;
        let preconf_router = read_contract_address("PRECONF_ROUTER_ADDRESS")?;
        let taiko_wrapper = read_contract_address("TAIKO_WRAPPER_ADDRESS")?;
        let forced_inclusion_store = read_contract_address("FORCED_INCLUSION_STORE_ADDRESS")?;
        let contract_addresses = L1ContractAddresses {
            preconf_whitelist,
            preconf_router,
            taiko_wrapper,
            forced_inclusion_store,
        };

        let handover_window_slots = std::env::var("HANDOVER_WINDOW_SLOTS")
            .unwrap_or("4".to_string())
            .parse::<u64>()
            .map_err(|e| anyhow::anyhow!("HANDOVER_WINDOW_SLOTS must be a number: {}", e))?;

        let handover_start_buffer_ms = std::env::var("HANDOVER_START_BUFFER_MS")
            .unwrap_or("6000".to_string())
            .parse::<u64>()
            .map_err(|e| anyhow::anyhow!("HANDOVER_START_BUFFER_MS must be a number: {}", e))?;

        let l1_height_lag = std::env::var("L1_HEIGHT_LAG")
            .unwrap_or("4".to_string())
            .parse::<u64>()
            .map_err(|e| anyhow::anyhow!("L1_HEIGHT_LAG must be a number: {}", e))?;

        let propose_forced_inclusion = std::env::var("PROPOSE_FORCED_INCLUSION")
            .unwrap_or("true".to_string())
            .parse::<bool>()
            .map_err(|e| anyhow::anyhow!("PROPOSE_FORCED_INCLUSION must be a boolean: {}", e))?;

        let simulate_not_submitting_at_the_end_of_epoch =
            std::env::var("SIMULATE_NOT_SUBMITTING_AT_THE_END_OF_EPOCH")
                .unwrap_or("false".to_string())
                .parse::<bool>()
                .map_err(|e| {
                    anyhow::anyhow!(
                        "SIMULATE_NOT_SUBMITTING_AT_THE_END_OF_EPOCH must be a boolean: {}",
                        e
                    )
                })?;

        Ok(ShastaConfig {
            shasta_inbox,
            contract_addresses,
            handover_window_slots,
            handover_start_buffer_ms,
            l1_height_lag,
            propose_forced_inclusion,
            simulate_not_submitting_at_the_end_of_epoch,
        })
    }
}

use std::fmt;
impl fmt::Display for ShastaConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Shasta inbox: {:#?}", self.shasta_inbox)?;
        writeln!(f, "Contract addresses: {:#?}", self.contract_addresses)?;
        writeln!(f, "handover window slots: {}", self.handover_window_slots)?;
        writeln!(
            f,
            "handover start buffer: {}ms",
            self.handover_start_buffer_ms
        )?;
        writeln!(f, "l1 height lag: {}", self.l1_height_lag)?;
        writeln!(
            f,
            "propose forced inclusion: {}",
            self.propose_forced_inclusion
        )?;
        writeln!(
            f,
            "simulate not submitting at the end of epoch: {}",
            self.simulate_not_submitting_at_the_end_of_epoch
        )?;
        Ok(())
    }
}
