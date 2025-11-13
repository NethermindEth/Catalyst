use crate::{config::Config, fork_info::Fork};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ForkInfoConfig {
    pub initial_fork: Fork,
    pub fork_switch_timestamp: Option<Duration>,
    pub fork_switch_l2_height: Option<u64>,
    pub fork_switch_transition_period: Duration,
}

impl From<&Config> for ForkInfoConfig {
    fn from(config: &Config) -> Self {
        Self {
            initial_fork: config.initial_fork.clone(),
            fork_switch_timestamp: config.fork_switch_timestamp.map(Duration::from_secs),
            fork_switch_l2_height: config.fork_switch_l2_height,
            fork_switch_transition_period: Duration::from_secs(
                config.fork_switch_transition_period_sec,
            ),
        }
    }
}
