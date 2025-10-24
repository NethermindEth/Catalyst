use alloy::primitives::{Address, U256};
use anyhow::Error;

pub trait PreconferProvider {
    fn get_preconfer_alloy_address(&self) -> Address;
    // nonce
    fn get_preconfer_nonce_pending(&self) -> impl Future<Output = Result<u64, Error>> + Send;
    fn get_preconfer_nonce_latest(&self) -> impl Future<Output = Result<u64, Error>> + Send;
    // balance
    fn get_preconfer_wallet_eth(&self) -> impl Future<Output = Result<U256, Error>> + Send;
}
