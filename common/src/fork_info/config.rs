use crate::{config::Config, fork_info::Fork};

pub struct ForkInfoConfig {
    pub current_fork: Fork,
    pub fork_switch_timestamp: Option<u64>,
}

impl From<&Config> for ForkInfoConfig {
    fn from(config: &Config) -> Self {
        Self {
            current_fork: config.current_fork.clone(),
            fork_switch_timestamp: config.fork_switch_timestamp,
        }
    }
}
