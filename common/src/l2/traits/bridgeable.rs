use alloy::primitives::{Address, U256};
use anyhow::Error;
use std::future::Future;

pub trait Bridgeable {
    fn get_balance(&self, address: Address) -> impl Future<Output = Result<U256, Error>> + Send;
    fn transfer_eth_from_l2_to_l1(
        &self,
        amount: u128,
        chain_id: u64,
        address: Address,
        bridge_relayer_fee: u64,
    ) -> impl Future<Output = Result<(), Error>> + Send;
}
