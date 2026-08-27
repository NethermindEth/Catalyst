mod web3signer;
mod web3signer_info;

use alloy::primitives::Address;
use alloy::signers::local::PrivateKeySigner;
use anyhow::Error;
use std::str::FromStr;
use std::sync::Arc;
use web3signer::Web3Signer;
pub use web3signer::Web3TxSigner;
pub use web3signer_info::Web3SignerInfo;

pub enum Signer {
    Web3signer(Arc<Web3Signer>, Address),
    PrivateKey(String, Address),
}

pub async fn create_signer(
    web3signer_info: Option<Web3SignerInfo>,
    catalyst_node_ecdsa_private_key: Option<String>,
) -> Result<Arc<Signer>, Error> {
    Ok(Arc::new(if let Some(web3signer_info) = web3signer_info {
        let address = web3signer_info
            .signer_address
            .parse()
            .expect("signer address is required for web3signer usage");
        Signer::Web3signer(Arc::new(Web3Signer::new(web3signer_info).await?), address)
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
