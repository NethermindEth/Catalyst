use crate::{config::Config, fork_info::Fork};

pub struct ForkInfoConfig {
    pub initial_fork: Fork,
    pub fork_switch_timestamp: Option<u64>,
    pub fork_switch_l2_height: Option<u64>,
}

impl From<&Config> for ForkInfoConfig {
    fn from(config: &Config) -> Self {
        Self {
            initial_fork: config.initial_fork.clone(),
            fork_switch_timestamp: config.fork_switch_timestamp,
            fork_switch_l2_height: config.fork_switch_l2_height,
        }
    }
}
