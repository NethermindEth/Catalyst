use crate::l1::bindings::IRealTimeInbox::Config;

#[derive(Clone, Default)]
pub struct ProtocolConfig {
    pub basefee_sharing_pctg: u8,
}

impl From<&Config> for ProtocolConfig {
    fn from(config: &Config) -> Self {
        Self {
            basefee_sharing_pctg: config.basefeeSharingPctg,
        }
    }
}

impl ProtocolConfig {
    pub fn get_basefee_sharing_pctg(&self) -> u8 {
        self.basefee_sharing_pctg
    }
}
