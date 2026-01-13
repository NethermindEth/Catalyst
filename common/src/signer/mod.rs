pub mod web3signer;

use alloy::primitives::Address;
use alloy::signers::local::PrivateKeySigner;
use anyhow::Error;
use std::str::FromStr;
use std::sync::Arc;
use tokio::time::Duration;
use web3signer::Web3Signer;

#[derive(Debug)]
pub enum Signer {
    Web3signer(Arc<Web3Signer>, Address),
    PrivateKey(String, Address),
}

const SIGNER_TIMEOUT: Duration = Duration::from_secs(10);

pub async fn create_signer(
    web3signer_url: Option<String>,
    catalyst_node_ecdsa_private_key: Option<String>,
    preconfer_address: Option<Address>,
) -> Result<Arc<Signer>, Error> {
    Ok(Arc::new(if let Some(web3signer_url) = web3signer_url {
        let address =
            preconfer_address.expect("preconfer address is required for web3signer usage");
        Signer::Web3signer(
            Arc::new(Web3Signer::new(&web3signer_url, SIGNER_TIMEOUT, &address.to_string()).await?),
            address,
        )
    } else if let Some(catalyst_node_ecdsa_private_key) = catalyst_node_ecdsa_private_key {
        let signer = PrivateKeySigner::from_str(catalyst_node_ecdsa_private_key.as_str())?;
        Signer::PrivateKey(catalyst_node_ecdsa_private_key, signer.address())
    } else {
        panic!("No signer provided");
    }))
}

impl Signer {
    pub fn get_address(&self) -> Address {
        match self {
            Signer::Web3signer(_, address) => *address,
            Signer::PrivateKey(_, address) => *address,
        }
    }
}
