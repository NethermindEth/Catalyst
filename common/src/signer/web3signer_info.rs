use alloy::primitives::Address;
use anyhow::{Result, anyhow};
use std::path::PathBuf;
use std::time::Duration;

const SIGNER_TIMEOUT: Duration = Duration::from_secs(10);
pub struct Web3SignerInfo {
    pub url: String,
    pub timeout: Duration,
    pub signer_address: String,
    pub ca_cert: PathBuf,
    pub client_cert: PathBuf,
    pub client_key: PathBuf,
}

impl Web3SignerInfo {
    pub fn new(
        web3signer_url: Option<String>,
        web3signer_root_certificate_path: Option<String>,
        web3signer_client_certificate_path: Option<String>,
        web3signer_client_key_path: Option<String>,
        preconfer_address: Option<Address>,
    ) -> Result<Option<Self>> {
        let Some(url) = web3signer_url else {
            // Web3Signer is not configured.
            return Ok(None);
        };

        Ok(Some(Self {
            url,
            timeout: SIGNER_TIMEOUT,
            signer_address: preconfer_address
                .ok_or_else(|| anyhow!("preconfer_address is required when using Web3Signer"))?
                .to_string(),
            ca_cert: PathBuf::from(web3signer_root_certificate_path.ok_or_else(|| {
                anyhow!("web3signer_root_certificate_path is required when using Web3Signer")
            })?),
            client_cert: PathBuf::from(web3signer_client_certificate_path.ok_or_else(|| {
                anyhow!("web3signer_client_certificate_path is required when using Web3Signer")
            })?),
            client_key: PathBuf::from(web3signer_client_key_path.ok_or_else(|| {
                anyhow!("web3signer_client_key_path is required when using Web3Signer")
            })?),
        }))
    }
}
