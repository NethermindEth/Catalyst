use super::operators_cache::OperatorsCacheState;
use alloy::primitives::Address;
use anyhow::Error;
use std::future::Future;

pub trait PreconfOperator {
    fn get_preconfer_address(&self) -> Address;
    fn get_operators_for_current_and_next_epoch(
        &self,
        current_slot_timestamp: u64,
    ) -> impl Future<Output = Result<OperatorsCacheState, Error>> + Send;
    fn get_l2_height_from_taiko_inbox(&self) -> impl Future<Output = Result<u64, Error>> + Send;
}

pub trait WhitelistProvider: Send + Sync {
    fn is_operator_whitelisted(&self) -> impl Future<Output = Result<bool, Error>> + Send;
}
