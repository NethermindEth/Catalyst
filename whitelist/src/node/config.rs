use crate::utils;
use common::utils as common_utils;

#[derive(Debug, Clone)]
pub struct NodeConfig {
    pub preconf_heartbeat_ms: u64,
    pub handover_window_slots: u64,
    pub handover_start_buffer_ms: u64,
    pub l1_height_lag: u64,
    pub propose_forced_inclusion: bool,
    pub simulate_not_submitting_at_the_end_of_epoch: bool,
    pub fork_timestamp: u64,
}

impl From<common_utils::config::Config<utils::config::Config>> for NodeConfig {
    fn from(config: common_utils::config::Config<utils::config::Config>) -> Self {
        Self {
            preconf_heartbeat_ms: config.preconf_heartbeat_ms,
            handover_window_slots: config.specific_config.handover_window_slots,
            handover_start_buffer_ms: config.specific_config.handover_start_buffer_ms,
            l1_height_lag: config.specific_config.l1_height_lag,
            propose_forced_inclusion: config.specific_config.propose_forced_inclusion,
            simulate_not_submitting_at_the_end_of_epoch: config
                .specific_config
                .simulate_not_submitting_at_the_end_of_epoch,
            fork_timestamp: config.specific_config.fork_switch_timestamp,
        }
    }
}
