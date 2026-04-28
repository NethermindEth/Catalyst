use crate::l1::bindings::IRealTimeInbox::Config;
use alloy::primitives::Address;

#[derive(Clone, Default)]
pub struct ProtocolConfig {
    pub basefee_sharing_pctg: u8,
    #[allow(dead_code)]
    pub proof_verifier: Address,
    #[allow(dead_code)]
    pub signal_service: Address,
}

impl From<&Config> for ProtocolConfig {
    fn from(config: &Config) -> Self {
        Self {
            basefee_sharing_pctg: config.basefeeSharingPctg,
            proof_verifier: config.proofVerifier,
            signal_service: config.signalService,
        }
    }
}

impl ProtocolConfig {
    pub fn get_basefee_sharing_pctg(&self) -> u8 {
        self.basefee_sharing_pctg
    }

    /// Use the EVM blockhash() 256-block limit as the max anchor height offset.
    #[allow(dead_code)]
    pub fn get_max_anchor_height_offset(&self) -> u64 {
        256
    }
}
