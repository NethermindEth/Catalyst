// TODO remove allow dead_code when the module is used
#![allow(dead_code)]

use crate::utils::config::{L1ContractAddresses, ShastaConfig};
use alloy::primitives::Address;

#[derive(Clone)]
pub struct ContractAddresses {
    pub shasta_inbox: Address,
    pub codec_address: Address,
}

impl TryFrom<L1ContractAddresses> for ContractAddresses {
    type Error = anyhow::Error;

    fn try_from(l1_contract_addresses: L1ContractAddresses) -> Result<Self, Self::Error> {
        Ok(ContractAddresses {
            shasta_inbox: l1_contract_addresses.shasta_inbox.parse()?,
            codec_address: l1_contract_addresses.codec_address.parse()?,
        })
    }
}

pub struct EthereumL1Config {
    pub contract_addresses: ContractAddresses,
}

impl TryFrom<ShastaConfig> for EthereumL1Config {
    type Error = anyhow::Error;

    fn try_from(config: ShastaConfig) -> Result<Self, Self::Error> {
        Ok(EthereumL1Config {
            contract_addresses: ContractAddresses::try_from(config.contract_addresses)?,
        })
    }
}
