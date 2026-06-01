use crate::l1::bindings::ProofType;
use crate::utils::config::RealtimeConfig;
use alloy::primitives::Address;

#[derive(Clone)]
pub struct ContractAddresses {
    pub realtime_inbox: Address,
    pub proposer_multicall: Address,
    pub bridge: Address,
}

pub struct EthereumL1Config {
    pub realtime_inbox: Address,
    pub proposer_multicall: Address,
    pub bridge: Address,
    pub proof_type: ProofType,
    pub mock_mode: bool,
    pub privacy_mode: bool,
    pub privacy_symmetric_key: Option<[u8; 32]>,
    pub fi_max_per_proposal: u16,
}

impl TryFrom<RealtimeConfig> for EthereumL1Config {
    type Error = anyhow::Error;

    fn try_from(config: RealtimeConfig) -> Result<Self, Self::Error> {
        Ok(EthereumL1Config {
            realtime_inbox: config.realtime_inbox,
            proposer_multicall: config.proposer_multicall,
            bridge: config.bridge,
            proof_type: config.proof_type,
            mock_mode: config.mock_mode,
            privacy_mode: config.privacy_mode,
            privacy_symmetric_key: config.privacy_symmetric_key,
            fi_max_per_proposal: config.fi_max_per_proposal,
        })
    }
}
