use crate::config::Config;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ForkInfoConfig {
    pub fork_switch_timestamps: Vec<Duration>,
    pub fork_switch_transition_period: Duration,
}

impl Default for ForkInfoConfig {
    fn default() -> Self {
        Self {
            fork_switch_timestamps: vec![
                Duration::from_secs(0),           // Pacaya
                Duration::from_secs(99999999999), // Shasta
            ],
            fork_switch_transition_period: Duration::from_secs(15),
        }
    }
}

impl From<&Config> for ForkInfoConfig {
    fn from(config: &Config) -> Self {
        Self {
            fork_switch_timestamps: vec![
                Duration::from_secs(config.pacaya_timestamp_sec),
                Duration::from_secs(config.shasta_timestamp_sec),
            ],
            fork_switch_transition_period: Duration::from_secs(
                config.fork_switch_transition_period_sec,
            ),
        }
    }
}
