use common::config::ConfigTrait;
use tracing::warn;

#[derive(Debug, Clone)]
pub struct L1ContractAddresses {
    pub shasta_inbox: String,
}

#[derive(Debug, Clone)]
pub struct ShastaConfig {
    pub shasta_inbox: String,
}

impl ConfigTrait for ShastaConfig {
    fn read_env_variables() -> Self {
        let default_empty_address = "0x0000000000000000000000000000000000000000".to_string();
        let read_contract_address = |env_var: &str, contract_name: &str| {
            std::env::var(env_var).unwrap_or_else(|_| {
                warn!(
                    "No {} contract address found in {} env var, using default",
                    contract_name, env_var
                );
                default_empty_address.clone()
            })
        };

        let shasta_inbox = read_contract_address("SHASTA_INBOX_ADDRESS", "ShastaInbox");

        ShastaConfig { shasta_inbox }
    }
}

use std::fmt;
impl fmt::Display for ShastaConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Shasta inbox: {:#?}", self.shasta_inbox)?;
        Ok(())
    }
}
