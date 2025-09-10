pub mod web3signer;

use anyhow::Error;
use std::sync::Arc;
use tokio::time::Duration;
use web3signer::Web3Signer;

#[derive(Debug)]
pub enum Signer {
    Web3signer(Arc<Web3Signer>),
    PrivateKey(String),
}

const SIGNER_TIMEOUT: Duration = Duration::from_secs(10);

pub async fn create_signer(
    web3signer_url: Option<String>,
    catalyst_node_ecdsa_private_key: Option<String>,
    preconfer_address: Option<String>,
) -> Result<Arc<Signer>, Error> {
    Ok(Arc::new(if let Some(web3signer_url) = web3signer_url {
        Signer::Web3signer(Arc::new(
            Web3Signer::new(
                &web3signer_url,
                SIGNER_TIMEOUT,
                preconfer_address
                    .as_ref()
                    .expect("preconfer address is required for web3signer usage"),
            )
            .await?,
        ))
    } else if let Some(catalyst_node_ecdsa_private_key) = catalyst_node_ecdsa_private_key {
        Signer::PrivateKey(catalyst_node_ecdsa_private_key)
    } else {
        panic!("No signer provided");
    }))
}
