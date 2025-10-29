// TODO remove allow dead_code when the module is used
#![allow(dead_code)]

use crate::utils::config::ShastaConfig;
use alloy::primitives::Address;

#[derive(Clone)]
pub struct ContractAddresses {
    pub shasta_inbox: Address,
    pub codec: Address,
    pub proposer_checker: Address,
}

pub struct EthereumL1Config {
    pub shasta_inbox: Address,
}

impl TryFrom<ShastaConfig> for EthereumL1Config {
    type Error = anyhow::Error;

    fn try_from(config: ShastaConfig) -> Result<Self, Self::Error> {
        Ok(EthereumL1Config {
            shasta_inbox: config.shasta_inbox.parse()?,
        })
    }
}
