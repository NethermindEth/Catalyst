mod config;
mod fork;
use anyhow::Error;
use config::ForkInfoConfig;
pub use fork::Fork;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct ForkInfo {
    pub fork: Fork,
    pub config: ForkInfoConfig,
}

impl ForkInfo {
    pub fn from_config(config: ForkInfoConfig) -> Result<Self, Error> {
        let fork = if Self::is_fork_switch_time(&config, 0)? {
            config.initial_fork.next().unwrap()
        } else {
            config.initial_fork.clone()
        };
        Ok(Self { fork, config })
    }

    pub fn is_next_fork_active(&self, l2_height: u64) -> Result<bool, Error> {
        if self.fork != self.config.initial_fork {
            return Ok(false);
        }
        Self::is_fork_switch_time(&self.config, l2_height)
    }

    fn is_fork_switch_time(config: &ForkInfoConfig, l2_height: u64) -> Result<bool, Error> {
        if let Some(fork_timestamp) = config.fork_switch_timestamp {
            let current_timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
            return Ok(current_timestamp >= fork_timestamp);
        }

        if let Some(fork_l2_height) = config.fork_switch_l2_height {
            return Ok(l2_height >= fork_l2_height - 1);
        }

        Ok(false)
    }
}
