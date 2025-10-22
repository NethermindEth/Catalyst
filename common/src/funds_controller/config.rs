use crate::config::Config;
use alloy::primitives::U256;
use std::time::Duration;

pub struct Thresholds {
    pub eth: U256,
    pub taiko: U256,
}

pub struct FundsControllerConfig {
    pub thresholds: Thresholds,
    pub amount_to_bridge_from_l2_to_l1: u128,
    pub disable_bridging: bool,
    pub bridge_relayer_fee: u64,
    pub bridge_transaction_fee: u64,
    pub monitor_interval: Duration,
}

impl From<&Config> for FundsControllerConfig {
    fn from(config: &Config) -> Self {
        Self {
            thresholds: Thresholds {
                eth: U256::from(config.threshold_eth),
                taiko: U256::from(config.threshold_taiko),
            },
            amount_to_bridge_from_l2_to_l1: config.amount_to_bridge_from_l2_to_l1,
            disable_bridging: config.disable_bridging,
            bridge_relayer_fee: config.bridge_relayer_fee,
            bridge_transaction_fee: config.bridge_transaction_fee,
            monitor_interval: Duration::from_secs(config.funds_monitor_interval_sec),
        }
    }
}
