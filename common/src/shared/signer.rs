use super::web3signer::Web3Signer;
use std::sync::Arc;

#[derive(Debug)]
pub enum Signer {
    Web3signer(Arc<Web3Signer>),
    PrivateKey(String),
}
