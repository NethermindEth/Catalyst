mod config;
mod fork;
use anyhow::Error;
use config::ForkInfoConfig;
pub use fork::Fork;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct ForkInfo {
    pub fork: Fork,
    pub config: ForkInfoConfig,
}

impl ForkInfo {
    pub fn from_config(config: ForkInfoConfig, l2_height: u64) -> Result<Self, Error> {
        let fork = if Self::is_fork_switch_time(&config, l2_height)?
            && let Some(next_fork) = config.initial_fork.next()
        {
            next_fork
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
            let current_timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?;
            return Ok(current_timestamp >= fork_timestamp);
        }

        if let Some(fork_l2_height) = config.fork_switch_l2_height {
            return Ok(l2_height >= fork_l2_height - 1);
        }

        Ok(false)
    }

    pub fn is_fork_switch_transition_period(&self, current_time: Duration) -> bool {
        if let Some(fork_timestamp) = self.config.fork_switch_timestamp {
            return current_time <= fork_timestamp
                && current_time >= fork_timestamp - self.config.fork_switch_transition_period;
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_fork_switch_transition_period() {
        let config = ForkInfoConfig {
            fork_switch_timestamp: Some(Duration::from_secs(10)),
            fork_switch_transition_period: Duration::from_secs(5),
            initial_fork: Fork::Pacaya,
            fork_switch_l2_height: None,
        };
        let fork_info = ForkInfo::from_config(config, 10).unwrap();
        assert!(fork_info.is_fork_switch_transition_period(Duration::from_secs(10)));
        assert!(fork_info.is_fork_switch_transition_period(Duration::from_secs(5)));
        assert!(!fork_info.is_fork_switch_transition_period(Duration::from_secs(11)));
        assert!(!fork_info.is_fork_switch_transition_period(Duration::from_secs(4)));
    }
}
