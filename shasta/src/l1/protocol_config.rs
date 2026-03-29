use taiko_bindings::inbox::IInbox::Config;
use taiko_protocol::shasta::constants::{
    max_anchor_offset_for_chain, timestamp_max_offset_for_chain,
};

#[derive(Clone, Default)]
pub struct ProtocolConfig {
    basefee_sharing_pctg: u8,
    max_anchor_offset: u64,
    timestamp_max_offset: u64,
}

impl ProtocolConfig {
    pub fn from(chain_id: u64, inbox_config: &Config) -> Self {
        Self {
            basefee_sharing_pctg: inbox_config.basefeeSharingPctg,
            max_anchor_offset: max_anchor_offset_for_chain(chain_id),
            timestamp_max_offset: timestamp_max_offset_for_chain(chain_id),
        }
    }

    pub fn get_basefee_sharing_pctg(&self) -> u8 {
        self.basefee_sharing_pctg
    }

    pub fn get_max_anchor_height_offset(&self) -> u64 {
        self.max_anchor_offset
    }

    pub fn get_timestamp_max_offset(&self) -> u64 {
        self.timestamp_max_offset
    }
}
