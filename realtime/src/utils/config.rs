use crate::l1::bindings::ProofType;
use alloy::primitives::Address;
use anyhow::Error;
use common::config::{ConfigTrait, address_parse_error};
use std::fmt;
use std::str::FromStr;

#[derive(Clone)]
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
    pub raiko_timeout_sec: u64,
    pub bridge_rpc_addr: String,
    pub user_op_status_db_path: String,
    pub preconf_only: bool,
    pub proof_request_bypass: bool,
    /// When true, overrides the SubProof bit flag to MOCK_ECDSA (0b00000001)
    /// regardless of `proof_type`. Allows using a real Raiko proof type string
    /// while routing on-chain to the DummyProofVerifier.
    pub mock_mode: bool,
    /// When true, the proposer encrypts every blob payload with AES-256-GCM under
    /// `privacy_symmetric_key` before posting to L1 (scheme 0x01). When false, blobs
    /// are emitted with the explicit plaintext scheme byte (0x00). Driver and raiko
    /// must be configured the same way.
    pub privacy_mode: bool,
    /// 32-byte AES-256-GCM key used by Catalyst when `privacy_mode == true`. Required
    /// in privacy mode; ignored otherwise.
    pub privacy_symmetric_key: Option<[u8; 32]>,
    /// Maximum number of forced inclusions to consume per proposal.
    pub fi_max_per_proposal: u16,
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

        let raiko_timeout_sec: u64 = std::env::var("RAIKO_TIMEOUT_SEC")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30);

        // Default to loopback so the unauthenticated surge_* JSON-RPC endpoints
        // are not exposed externally unless an operator opts in.
        let bridge_rpc_addr =
            std::env::var("BRIDGE_RPC_ADDR").unwrap_or_else(|_| "127.0.0.1:4545".to_string());

        let user_op_status_db_path = std::env::var("USER_OP_STATUS_DB_PATH")
            .unwrap_or_else(|_| "data/user_op_status".to_string());

        let preconf_only = std::env::var("PRECONF_ONLY")
            .map(|v| v.to_lowercase() != "false" && v != "0")
            .unwrap_or(true);

        let proof_request_bypass = std::env::var("PROOF_REQUEST_BYPASS")
            .map(|v| v.to_lowercase() != "false" && v != "0")
            .unwrap_or(false);

        let mock_mode = std::env::var("MOCK_MODE")
            .map(|v| v.to_lowercase() != "false" && v != "0")
            .unwrap_or(false);

        let privacy_mode = std::env::var("SURGE_PRIVACY_MODE")
            .map(|v| v.to_lowercase() != "false" && v != "0")
            .unwrap_or(false);

        let privacy_symmetric_key = match std::env::var("SURGE_PRIVACY_SYMMETRIC_KEY") {
            Ok(hex_str) => {
                let s = hex_str.strip_prefix("0x").unwrap_or(&hex_str);
                let bytes = hex::decode(s).map_err(|e| {
                    anyhow::anyhow!("SURGE_PRIVACY_SYMMETRIC_KEY: invalid hex: {e}")
                })?;
                if bytes.len() != 32 {
                    return Err(anyhow::anyhow!(
                        "SURGE_PRIVACY_SYMMETRIC_KEY: expected 32 bytes, got {}",
                        bytes.len()
                    ));
                }
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes);
                Some(arr)
            }
            Err(_) => None,
        };

        if privacy_mode && privacy_symmetric_key.is_none() {
            return Err(anyhow::anyhow!(
                "SURGE_PRIVACY_MODE=true requires SURGE_PRIVACY_SYMMETRIC_KEY to be set"
            ));
        }

        let fi_max_per_proposal: u16 = std::env::var("FI_MAX_PER_PROPOSAL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(4);

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
            raiko_timeout_sec,
            bridge_rpc_addr,
            user_op_status_db_path,
            preconf_only,
            proof_request_bypass,
            mock_mode,
            privacy_mode,
            privacy_symmetric_key,
            fi_max_per_proposal,
        })
    }
}

/// Manual `Debug` impl that redacts `privacy_symmetric_key`. The derived impl
/// would print the raw 32-byte key if a `RealtimeConfig` was ever logged via
/// `{:?}`; for a long-running process that's a leak waiting to happen.
impl fmt::Debug for RealtimeConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RealtimeConfig")
            .field("realtime_inbox", &self.realtime_inbox)
            .field("proposer_multicall", &self.proposer_multicall)
            .field("bridge", &self.bridge)
            .field("l2_signal_service", &self.l2_signal_service)
            .field("raiko_url", &self.raiko_url)
            .field(
                "raiko_api_key",
                &self.raiko_api_key.as_ref().map(|_| "<set>"),
            )
            .field("proof_type", &self.proof_type)
            .field("raiko_poll_interval_ms", &self.raiko_poll_interval_ms)
            .field("raiko_max_retries", &self.raiko_max_retries)
            .field("raiko_timeout_sec", &self.raiko_timeout_sec)
            .field("bridge_rpc_addr", &self.bridge_rpc_addr)
            .field("user_op_status_db_path", &self.user_op_status_db_path)
            .field("preconf_only", &self.preconf_only)
            .field("proof_request_bypass", &self.proof_request_bypass)
            .field("mock_mode", &self.mock_mode)
            .field("privacy_mode", &self.privacy_mode)
            .field(
                "privacy_symmetric_key",
                &self.privacy_symmetric_key.as_ref().map(|_| "<redacted>"),
            )
            .field("fi_max_per_proposal", &self.fi_max_per_proposal)
            .finish()
    }
}

impl fmt::Display for RealtimeConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "RealTime inbox: {:#?}", self.realtime_inbox)?;
        writeln!(f, "Proposer multicall: {:#?}", self.proposer_multicall)?;
        writeln!(f, "L1 bridge: {:#?}", self.bridge)?;
        writeln!(f, "L2 signal service: {:#?}", self.l2_signal_service)?;
        writeln!(f, "Raiko URL: {}", self.raiko_url)?;
        writeln!(f, "Raiko max retries: {}", self.raiko_max_retries)?;
        writeln!(f, "Raiko timeout: {}s", self.raiko_timeout_sec)?;
        writeln!(
            f,
            "Proof type: {} (bit flag: {})",
            self.proof_type,
            self.proof_type.proof_bit_flag()
        )?;
        writeln!(f, "Mock mode: {}", self.mock_mode)?;
        writeln!(f, "Bridge RPC addr: {}", self.bridge_rpc_addr)?;
        writeln!(f, "User op status DB path: {}", self.user_op_status_db_path)?;
        writeln!(f, "Preconf only: {}", self.preconf_only)?;
        writeln!(f, "Proof request bypass: {}", self.proof_request_bypass)?;
        writeln!(f, "Privacy mode: {}", self.privacy_mode)?;
        writeln!(
            f,
            "Privacy symmetric key: {}",
            if self.privacy_symmetric_key.is_some() {
                "<set>"
            } else {
                "<unset>"
            }
        )?;
        writeln!(f, "FI max per proposal: {}", self.fi_max_per_proposal)?;
        Ok(())
    }
}
