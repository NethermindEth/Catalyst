use taiko_bindings::inbox::IInbox::Config;

#[derive(Clone, Default)]
pub struct ProtocolConfig {
    basefee_sharing_pctg: u8,
    max_anchor_offset: u64,
}

impl ProtocolConfig {
    pub fn from(shasta_config: &Config) -> Self {
        Self {
            basefee_sharing_pctg: shasta_config.basefeeSharingPctg,
            max_anchor_offset: 100, // 128 by document
        }
    }

    pub fn get_basefee_sharing_pctg(&self) -> u8 {
        self.basefee_sharing_pctg
    }

    pub fn get_max_anchor_height_offset(&self) -> u64 {
        self.max_anchor_offset
    }
}
