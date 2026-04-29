use crate::l1::bindings::ProofType;
use alloy::primitives::Address;
use anyhow::Error;
use common::config::{ConfigTrait, address_parse_error};
use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct RealtimeConfig {
    pub realtime_inbox: Address,
    pub proposer_multicall: Address,
    pub bridge: Address,
    /// L2 SignalService address — used on the L2 side for signal operations.
    pub l2_signal_service: Address,
    pub raiko_url: String,
    pub raiko_api_key: Option<String>,
    pub proof_type: ProofType,
    pub raiko_poll_interval_ms: u64,
    pub raiko_max_retries: u32,
    pub bridge_rpc_addr: String,
    pub preconf_only: bool,
    pub proof_request_bypass: bool,
    /// When true, overrides the SubProof bit flag to MOCK_ECDSA (0b00000001)
    /// regardless of `proof_type`. Allows using a real Raiko proof type string
    /// while routing on-chain to the DummyProofVerifier.
    pub mock_mode: bool,
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
        let l2_signal_service = read_contract_address("L2_SIGNAL_SERVICE_ADDRESS")?;

        let raiko_url =
            std::env::var("RAIKO_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());
        let raiko_api_key = std::env::var("RAIKO_API_KEY").ok();
        let proof_type: ProofType = std::env::var("PROOF_TYPE")
            .unwrap_or_else(|_| "sp1".to_string())
            .parse()?;

        let raiko_poll_interval_ms: u64 = std::env::var("RAIKO_POLL_INTERVAL_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2000);

        let raiko_max_retries: u32 = std::env::var("RAIKO_MAX_RETRIES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60);

        let bridge_rpc_addr =
            std::env::var("BRIDGE_RPC_ADDR").unwrap_or_else(|_| "0.0.0.0:4545".to_string());

        let preconf_only = std::env::var("PRECONF_ONLY")
            .map(|v| v.to_lowercase() != "false" && v != "0")
            .unwrap_or(true);

        let proof_request_bypass = std::env::var("PROOF_REQUEST_BYPASS")
            .map(|v| v.to_lowercase() != "false" && v != "0")
            .unwrap_or(false);

        let mock_mode = std::env::var("MOCK_MODE")
            .map(|v| v.to_lowercase() != "false" && v != "0")
            .unwrap_or(false);

        Ok(RealtimeConfig {
            realtime_inbox,
            proposer_multicall,
            bridge,
            l2_signal_service,
            raiko_url,
            raiko_api_key,
            proof_type,
            raiko_poll_interval_ms,
            raiko_max_retries,
            bridge_rpc_addr,
            preconf_only,
            proof_request_bypass,
            mock_mode,
        })
    }
}

use std::fmt;
impl fmt::Display for RealtimeConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "RealTime inbox: {:#?}", self.realtime_inbox)?;
        writeln!(f, "Proposer multicall: {:#?}", self.proposer_multicall)?;
        writeln!(f, "Raiko URL: {}", self.raiko_url)?;
        writeln!(
            f,
            "Proof type: {} (bit flag: {})",
            self.proof_type,
            self.proof_type.proof_bit_flag()
        )?;
        writeln!(f, "Preconf only: {}", self.preconf_only)?;
        writeln!(f, "Proof request bypass: {}", self.proof_request_bypass)?;
        Ok(())
    }
}
