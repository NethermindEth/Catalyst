use alloy::primitives::Address;

pub struct NodeConfig {
    pub preconf_heartbeat_ms: u64,
    pub coinbase: Address,
    pub l1_height_lag: u64,
}
