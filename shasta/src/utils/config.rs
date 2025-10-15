use common::config::ConfigTrait;
use tracing::warn;

#[derive(Debug, Clone)]
pub struct L1ContractAddresses {
    pub shasta_inbox: String,
}

#[derive(Debug, Clone)]
pub struct ShastaConfig {
    pub contract_addresses: L1ContractAddresses,
}

impl ConfigTrait for ShastaConfig {
    fn read_env_variables() -> Self {
        let default_empty_address = "0x0000000000000000000000000000000000000000".to_string();

        let shasta_inbox = std::env::var("SHASTA_INBOX_ADDRESS").unwrap_or_else(|_| {
            warn!("No Shasta inbox address found in SHASTA_INBOX_ADDRESS env var, using default");
            default_empty_address.clone()
        });
        let contract_addresses = L1ContractAddresses { shasta_inbox };

        ShastaConfig { contract_addresses }
    }
}

use std::fmt;
impl fmt::Display for ShastaConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Contract addresses: {:#?}", self.contract_addresses)?;
        Ok(())
    }
}
