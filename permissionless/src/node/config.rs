use alloy::primitives::Address;
use secp256k1::SecretKey;

pub struct NodeConfig {
    pub preconf_heartbeat_ms: u64,
    pub coinbase: Address,
    pub l1_height_lag: u64,
    pub min_anchor_offset: u64,
    pub sequencer_key: SecretKey,
    pub watchdog_max_counter: u64,
}
