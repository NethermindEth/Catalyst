use super::bindings::Inbox::Config;

#[derive(Clone, Default)]
pub struct ProtocolConfig {
    basefee_sharing_pctg: u8,
    // TODO initialize these values correctly
    _min_anchor_offset: u64,
    max_anchor_offset: u64,
}

impl ProtocolConfig {
    pub fn from(shasta_config: &Config) -> Self {
        Self {
            basefee_sharing_pctg: shasta_config.basefeeSharingPctg,
            _min_anchor_offset: 2, // https://github.com/taikoxyz/taiko-mono/blob/main/packages/protocol/docs/Derivation.md#constants
            max_anchor_offset: 100, // 128 by document
        }
    }

    pub fn get_basefee_sharing_pctg(&self) -> u8 {
        self.basefee_sharing_pctg
    }

    #[allow(dead_code)]
    pub fn get_min_anchor_height_offset(&self) -> u64 {
        self._min_anchor_offset
    }

    pub fn get_max_anchor_height_offset(&self) -> u64 {
        self.max_anchor_offset
    }
}
