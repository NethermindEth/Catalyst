use alloy::primitives::Address;
use anyhow::Error;
use common::config::{ConfigTrait, address_parse_error};
use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct RealtimeConfig {
    pub realtime_inbox: Address,
    pub proposer_multicall: Address,
    pub bridge: Address,
    pub raiko_url: String,
    pub raiko_api_key: Option<String>,
    pub proof_type: String,
    pub raiko_network: String,
    pub raiko_l1_network: String,
    pub preconf_only: bool,
    pub proof_request_bypass: bool,
}

impl ConfigTrait for RealtimeConfig {
    fn read_env_variables() -> Result<Self, Error> {
        let read_contract_address = |env_var: &str| -> Result<Address, Error> {
            let address_str = std::env::var(env_var)
                .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", env_var, e))?;
            Address::from_str(&address_str)
                .map_err(|e| address_parse_error(env_var, e, &address_str))
        };

        let realtime_inbox = read_contract_address("REALTIME_INBOX_ADDRESS")?;
        let proposer_multicall = read_contract_address("PROPOSER_MULTICALL_ADDRESS")?;
        let bridge = read_contract_address("L1_BRIDGE_ADDRESS")?;

        let raiko_url = std::env::var("RAIKO_URL")
            .unwrap_or_else(|_| "http://localhost:8080".to_string());
        let raiko_api_key = std::env::var("RAIKO_API_KEY").ok();
        let proof_type = std::env::var("RAIKO_PROOF_TYPE")
            .unwrap_or_else(|_| "sgx".to_string());
        let raiko_network = std::env::var("RAIKO_L2_NETWORK")
            .unwrap_or_else(|_| "taiko_mainnet".to_string());
        let raiko_l1_network = std::env::var("RAIKO_L1_NETWORK")
            .unwrap_or_else(|_| "ethereum".to_string());

        let preconf_only = std::env::var("PRECONF_ONLY")
            .map(|v| v.to_lowercase() != "false" && v != "0")
            .unwrap_or(true);

        let proof_request_bypass = std::env::var("PROOF_REQUEST_BYPASS")
            .map(|v| v.to_lowercase() != "false" && v != "0")
            .unwrap_or(false);

        Ok(RealtimeConfig {
            realtime_inbox,
            proposer_multicall,
            bridge,
            raiko_url,
            raiko_api_key,
            proof_type,
            raiko_network,
            raiko_l1_network,
            preconf_only,
            proof_request_bypass,
        })
    }
}

use std::fmt;
impl fmt::Display for RealtimeConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "RealTime inbox: {:#?}", self.realtime_inbox)?;
        writeln!(f, "Proposer multicall: {:#?}", self.proposer_multicall)?;
        writeln!(f, "Raiko URL: {}", self.raiko_url)?;
        writeln!(f, "Proof type: {}", self.proof_type)?;
        writeln!(f, "Preconf only: {}", self.preconf_only)?;
        writeln!(f, "Proof request bypass: {}", self.proof_request_bypass)?;
        Ok(())
    }
}
