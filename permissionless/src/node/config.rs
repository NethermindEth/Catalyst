use crate::utils;
use common::utils as common_utils;

pub struct NodeConfig {
    pub preconf_heartbeat_ms: u64,
}

impl From<common_utils::config::Config<utils::config::Config>> for NodeConfig {
    fn from(config: common_utils::config::Config<utils::config::Config>) -> Self {
        Self {
            preconf_heartbeat_ms: config.preconf_heartbeat_ms,
        }
    }
}
