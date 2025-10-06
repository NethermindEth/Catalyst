use common::shared::fork::Fork;
use common::utils::config_trait::ConfigTrait;
use tracing::warn;

#[derive(Debug, Clone)]
pub struct L1ContractAddresses {
    pub taiko_inbox: String,
    pub preconf_whitelist: String,
    pub preconf_router: String,
    pub taiko_wrapper: String,
    pub forced_inclusion_store: String,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub contract_addresses: L1ContractAddresses,
    pub handover_window_slots: u64,
    pub handover_start_buffer_ms: u64,
    pub l1_height_lag: u64,
    pub propose_forced_inclusion: bool,
    pub simulate_not_submitting_at_the_end_of_epoch: bool,
    pub fork: Fork,
}

impl ConfigTrait for Config {
    fn read_env_variables() -> Self {
        let default_empty_address = "0x0000000000000000000000000000000000000000".to_string();

        // Helper function to read contract address from environment variable
        let read_contract_address = |env_var: &str, contract_name: &str| {
            std::env::var(env_var).unwrap_or_else(|_| {
                warn!(
                    "No {} contract address found in {} env var, using default",
                    contract_name, env_var
                );
                default_empty_address.clone()
            })
        };

        let taiko_inbox = read_contract_address("TAIKO_INBOX_ADDRESS", "TaikoL1");
        let preconf_whitelist =
            read_contract_address("PRECONF_WHITELIST_ADDRESS", "PreconfWhitelist");
        let preconf_router = read_contract_address("PRECONF_ROUTER_ADDRESS", "PreconfRouter");
        let taiko_wrapper = read_contract_address("TAIKO_WRAPPER_ADDRESS", "TaikoWrapper");
        let forced_inclusion_store =
            read_contract_address("FORCED_INCLUSION_STORE_ADDRESS", "ForcedInclusionStore");

        let handover_window_slots = std::env::var("HANDOVER_WINDOW_SLOTS")
            .unwrap_or("4".to_string())
            .parse::<u64>()
            .expect("HANDOVER_WINDOW_SLOTS must be a number");

        let handover_start_buffer_ms = std::env::var("HANDOVER_START_BUFFER_MS")
            .unwrap_or("6000".to_string())
            .parse::<u64>()
            .expect("HANDOVER_START_BUFFER_MS must be a number");

        let l1_height_lag = std::env::var("L1_HEIGHT_LAG")
            .unwrap_or("4".to_string())
            .parse::<u64>()
            .expect("L1_HEIGHT_LAG must be a number");

        let propose_forced_inclusion = std::env::var("PROPOSE_FORCED_INCLUSION")
            .unwrap_or("true".to_string())
            .parse::<bool>()
            .expect("PROPOSE_FORCED_INCLUSION must be a boolean");

        let simulate_not_submitting_at_the_end_of_epoch =
            std::env::var("SIMULATE_NOT_SUBMITTING_AT_THE_END_OF_EPOCH")
                .unwrap_or("false".to_string())
                .parse::<bool>()
                .expect("SIMULATE_NOT_SUBMITTING_AT_THE_END_OF_EPOCH must be a boolean");

        let fork = std::env::var("FORK")
            .unwrap_or("pacaya".to_string())
            .parse::<Fork>()
            .expect("FORK must be a valid fork");

        Config {
            contract_addresses: L1ContractAddresses {
                taiko_inbox,
                preconf_whitelist,
                preconf_router,
                taiko_wrapper,
                forced_inclusion_store,
            },
            handover_window_slots,
            handover_start_buffer_ms,
            l1_height_lag,
            propose_forced_inclusion,
            simulate_not_submitting_at_the_end_of_epoch,
            fork,
        }
    }
}

use std::fmt;
impl fmt::Display for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
        writeln!(f, "fork: {}", self.fork)?;
        Ok(())
    }
}
