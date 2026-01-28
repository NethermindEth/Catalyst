use alloy::primitives::Address;
use anyhow::Error;
use common::config::{ConfigTrait, address_parse_error};
use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct ShastaConfig {
    pub shasta_inbox: Address,
    pub proposer_multicall: Address,
    pub bridge: Address,
}

impl ConfigTrait for ShastaConfig {
    fn read_env_variables() -> Result<Self, Error> {
        let read_contract_address = |env_var: &str| -> Result<Address, Error> {
            let address_str = std::env::var(env_var)
                .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", env_var, e))?;
            Address::from_str(&address_str)
                .map_err(|e| address_parse_error(env_var, e, &address_str))
        };

        let shasta_inbox = read_contract_address("SHASTA_INBOX_ADDRESS")?;
        let proposer_multicall = read_contract_address("PROPOSER_MULTICALL_ADDRESS")?;
        let bridge = read_contract_address("L1_BRIDGE_ADDRESS")?;

        Ok(ShastaConfig {
            shasta_inbox,
            proposer_multicall,
            bridge,
        })
    }
}

use std::fmt;
impl fmt::Display for ShastaConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Shasta inbox: {:#?}", self.shasta_inbox)?;
        writeln!(f, "Proposer multicall: {:#?}", self.proposer_multicall)?;
        Ok(())
    }
}
