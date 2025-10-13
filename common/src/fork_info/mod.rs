mod config;
mod fork;
use anyhow::Error;
use config::ForkInfoConfig;
pub use fork::Fork;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct ForkInfo {
    pub fork: Fork,
    pub switch_timestamp: Option<u64>,
}

impl ForkInfo {
    pub fn from_config(config: ForkInfoConfig) -> Result<Self, Error> {
        let current_fork = config.current_fork;
        let fork_switch_timestamp = config.fork_switch_timestamp;

        if Self::is_next_fork_active(fork_switch_timestamp)? {
            let next_fork = current_fork.next().ok_or_else(|| {
                anyhow::anyhow!("FORK_SWITCH_TIMESTAMP is set but there is no next fork")
            })?;

            Ok(Self {
                fork: next_fork,
                switch_timestamp: None,
            })
        } else {
            Ok(Self {
                fork: current_fork,
                switch_timestamp: fork_switch_timestamp,
            })
        }
    }

    pub fn is_next_fork_active(next_fork_timestamp: Option<u64>) -> Result<bool, Error> {
        if let Some(fork_timestamp) = next_fork_timestamp {
            let current_timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
            return Ok(current_timestamp >= fork_timestamp);
        }
        Ok(false)
    }
}
