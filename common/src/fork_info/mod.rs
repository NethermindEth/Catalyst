pub mod config;
pub mod fork;
use anyhow::Error;
use config::ForkInfoConfig;
pub use fork::Fork;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use strum::IntoEnumIterator;

#[derive(Debug, Clone)]
pub struct ForkInfo {
    pub fork: Fork,
    pub config: ForkInfoConfig,
}

impl Default for ForkInfo {
    fn default() -> Self {
        Self {
            fork: Fork::Pacaya,
            config: ForkInfoConfig::default(),
        }
    }
}

impl ForkInfo {
    pub fn from_config(config: ForkInfoConfig) -> Result<Self, Error> {
        let current_timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?;
        // TODO: consider changing l2_slot_info timestamp to Duration
        let fork = Self::choose_current_fork(&config, current_timestamp.as_secs())?;
        Ok(Self { fork, config })
    }

    pub fn is_next_fork_active(&self, timestamp_sec: u64) -> Result<bool, Error> {
        Ok(self.fork != Self::choose_current_fork(&self.config, timestamp_sec)?)
    }

    fn choose_current_fork(
        config: &ForkInfoConfig,
        current_timestamp_sec: u64,
    ) -> Result<Fork, Error> {
        let current_timestamp = Duration::from_secs(current_timestamp_sec);
        // Iterate through Fork variants in reverse order to find the highest fork that should be active
        for (fork_index, fork) in Fork::iter().enumerate().rev() {
            if let Some(&fork_timestamp) = config.fork_switch_timestamps.get(fork_index)
                && current_timestamp >= fork_timestamp
            {
                return Ok(fork);
            }
        }

        Err(anyhow::anyhow!("No fork found for current timestamp"))
    }

    pub fn is_fork_switch_transition_period(&self, current_time: Duration) -> bool {
        let current_fork_index = Fork::iter()
            .position(|f| f == self.fork)
            .expect("Fork should always be found in its own iterator");

        if let Some(next_fork_timestamp) = self
            .config
            .fork_switch_timestamps
            .get(current_fork_index + 1)
        {
            return current_time <= *next_fork_timestamp
                && current_time
                    >= *next_fork_timestamp - self.config.fork_switch_transition_period;
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
            fork_switch_timestamps: vec![
                Duration::from_secs(0),  // Pacaya
                Duration::from_secs(10), // Shasta
            ],
            fork_switch_transition_period: Duration::from_secs(5),
        };
        let mut fork_info = ForkInfo::from_config(config).unwrap();
        // Set fork to Pacaya to test transition to Shasta
        fork_info.fork = Fork::Pacaya;
        assert!(fork_info.is_fork_switch_transition_period(Duration::from_secs(10)));
        assert!(fork_info.is_fork_switch_transition_period(Duration::from_secs(5)));
        assert!(!fork_info.is_fork_switch_transition_period(Duration::from_secs(11)));
        assert!(!fork_info.is_fork_switch_transition_period(Duration::from_secs(4)));
    }
}
