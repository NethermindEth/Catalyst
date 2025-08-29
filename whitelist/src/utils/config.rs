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
}

impl ConfigTrait for Config {
    fn read_env_variables() -> Self {
        let default_empty_address = "0x0000000000000000000000000000000000000000".to_string();

        const TAIKO_INBOX_ADDRESS: &str = "TAIKO_INBOX_ADDRESS";
        let taiko_inbox = std::env::var(TAIKO_INBOX_ADDRESS).unwrap_or_else(|_| {
            warn!(
                "No TaikoL1 contract address found in {} env var, using default",
                TAIKO_INBOX_ADDRESS
            );
            default_empty_address.clone()
        });

        const PRECONF_WHITELIST_ADDRESS: &str = "PRECONF_WHITELIST_ADDRESS";
        let preconf_whitelist = std::env::var(PRECONF_WHITELIST_ADDRESS).unwrap_or_else(|_| {
            warn!(
                "No PreconfWhitelist contract address found in {} env var, using default",
                PRECONF_WHITELIST_ADDRESS
            );
            default_empty_address.clone()
        });

        const PRECONF_ROUTER_ADDRESS: &str = "PRECONF_ROUTER_ADDRESS";
        let preconf_router = std::env::var(PRECONF_ROUTER_ADDRESS).unwrap_or_else(|_| {
            warn!(
                "No PreconfRouter contract address found in {} env var, using default",
                PRECONF_ROUTER_ADDRESS
            );
            default_empty_address.clone()
        });

        const TAIKO_WRAPPER_ADDRESS: &str = "TAIKO_WRAPPER_ADDRESS";
        let taiko_wrapper = std::env::var(TAIKO_WRAPPER_ADDRESS).unwrap_or_else(|_| {
            warn!(
                "No TaikoWrapper contract address found in {} env var, using default",
                TAIKO_WRAPPER_ADDRESS
            );
            default_empty_address.clone()
        });

        const FORCED_INCLUSION_STORE_ADDRESS: &str = "FORCED_INCLUSION_STORE_ADDRESS";
        let forced_inclusion_store =
            std::env::var(FORCED_INCLUSION_STORE_ADDRESS).unwrap_or_else(|_| {
                warn!(
                    "No ForcedInclusionStore contract address found in {} env var, using default",
                    FORCED_INCLUSION_STORE_ADDRESS
                );
                default_empty_address.clone()
            });

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

        Ok(())
    }
}
