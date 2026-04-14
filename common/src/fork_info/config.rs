use super::fork::Fork;
use crate::config::Config;
use std::time::Duration;
use strum::IntoEnumIterator;

#[derive(Debug, Clone)]
pub struct ForkInfoConfig {
    pub fork_switch_timestamps: Vec<Duration>,
    pub fork_switch_transition_period: Duration,
}

impl Default for ForkInfoConfig {
    fn default() -> Self {
        Self {
            fork_switch_timestamps: vec![
                Duration::from_secs(0),           // Shasta
                Duration::from_secs(99999999999), // Permissionless
                Duration::from_secs(99999999999), // Realtime
            ],
            fork_switch_transition_period: Duration::from_secs(15),
        }
    }
}

impl From<&Config> for ForkInfoConfig {
    fn from(config: &Config) -> Self {
        let fork_switch_timestamps = Fork::iter()
            .map(|f| match f {
                Fork::Shasta => Duration::from_secs(config.shasta_timestamp_sec),
                Fork::Permissionless => Duration::from_secs(config.permissionless_timestamp_sec),
                Fork::Realtime => Duration::from_secs(99999999999), // Only activated via FORK=realtime
            })
            .collect();
        Self {
            fork_switch_timestamps,
            fork_switch_transition_period: Duration::from_secs(
                config.fork_switch_transition_period_sec,
            ),
        }
    }
}
