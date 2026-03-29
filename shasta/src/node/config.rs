#[derive(Debug, Clone)]
pub struct NodeConfig {
    pub preconf_heartbeat_ms: u64,
    pub handover_window_slots: u64,
    pub handover_start_buffer_ms: u64,
    pub l1_height_lag: u64,
    pub min_anchor_offset: u64,
    pub propose_forced_inclusion: bool,
    pub simulate_not_submitting_at_the_end_of_epoch: bool,
    pub max_blocks_to_reanchor: u64,
    pub watchdog_max_counter: u64,
}
