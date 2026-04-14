use crate::l1::bindings::ProofType;
use crate::raiko::RaikoClient;
use crate::utils::config::RealtimeConfig;
use alloy::primitives::Address;

#[derive(Clone)]
pub struct ContractAddresses {
    pub realtime_inbox: Address,
    pub proposer_multicall: Address,
    pub bridge: Address,
    pub signal_service: Address,
}

pub struct EthereumL1Config {
    pub realtime_inbox: Address,
    pub proposer_multicall: Address,
    pub bridge: Address,
    pub signal_service: Address,
    pub proof_type: ProofType,
    pub raiko_client: RaikoClient,
}

impl TryFrom<RealtimeConfig> for EthereumL1Config {
    type Error = anyhow::Error;

    fn try_from(config: RealtimeConfig) -> Result<Self, Self::Error> {
        let raiko_client = RaikoClient::new(&config);
        Ok(EthereumL1Config {
            realtime_inbox: config.realtime_inbox,
            proposer_multicall: config.proposer_multicall,
            bridge: config.bridge,
            signal_service: config.signal_service,
            proof_type: config.proof_type,
            raiko_client,
        })
    }
}
