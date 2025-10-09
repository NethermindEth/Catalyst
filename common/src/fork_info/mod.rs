mod fork;
use anyhow::Error;
pub use fork::Fork;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct ForkInfo {
    pub fork: Fork,
    pub switch_timestamp: Option<u64>,
}

impl ForkInfo {
    pub fn from_env() -> Result<Self, Error> {
        let current_fork = Self::parse_current_fork()?;
        let fork_switch_timestamp = Self::parse_fork_switch_timestamp()?;

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

    fn parse_current_fork() -> Result<Fork, Error> {
        std::env::var("CURRENT_FORK")
            .unwrap_or("pacaya".to_string())
            .parse::<Fork>()
            .map_err(|_| anyhow::anyhow!("CURRENT_FORK must be a valid fork"))
    }

    fn parse_fork_switch_timestamp() -> Result<Option<u64>, Error> {
        match std::env::var("FORK_SWITCH_TIMESTAMP") {
            Err(_) => Ok(None),
            Ok(timestamp) => {
                let v = timestamp
                    .parse::<u64>()
                    .map_err(|_| anyhow::anyhow!("FORK_SWITCH_TIMESTAMP must be a number"))?;
                Ok(Some(v))
            }
        }
    }
}
