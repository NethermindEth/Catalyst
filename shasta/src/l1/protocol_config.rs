use taiko_bindings::i_inbox::IInbox::Config;

#[derive(Clone, Default)]
pub struct ProtocolConfig {
    basefee_sharing_pctg: u8,
}

impl ProtocolConfig {
    pub fn from(shasta_config: &Config) -> Self {
        Self {
            basefee_sharing_pctg: shasta_config.basefeeSharingPctg,
        }
    }

    pub fn get_basefee_sharing_pctg(&self) -> u8 {
        self.basefee_sharing_pctg
    }
}
