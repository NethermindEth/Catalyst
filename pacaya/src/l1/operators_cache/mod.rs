use crate::l1::bindings::PreconfWhitelist;
use alloy::{
    primitives::Address,
    providers::{DynProvider, Provider},
};
use anyhow::Error;
use std::sync::RwLock;

mod error;
mod state;

use error::OperatorsCacheError;
pub use state::{Operators, OperatorsCacheState};

/// if latest block is older than this, node is stuck
const MAX_BLOCK_AGE_SECS: u64 = 60;

/// Cached result of get_operators_for_current_and_next_epoch.
/// Operators only change once per L1 slot (12s), so we avoid repeating the RPC call every L2 slot (2s).
/// Key is current_slot_timestamp.
pub struct OperatorsCache {
    cache: RwLock<Option<OperatorsCacheState>>,
    provider: DynProvider,
    whitelist_address: Address,
}

impl OperatorsCache {
    pub fn new(provider: DynProvider, whitelist_address: Address) -> Self {
        Self {
            cache: RwLock::new(None),
            provider,
            whitelist_address,
        }
    }

    pub async fn get_operators_for_current_and_next_epoch(
        &self,
        current_slot_timestamp: u64,
    ) -> Result<OperatorsCacheState, Error> {
        if let Some(cached) = self.read_cached_state() {
            if cached.timestamp() == current_slot_timestamp {
                return Ok(cached);
            } else if cached.timestamp().saturating_add(MAX_BLOCK_AGE_SECS) < current_slot_timestamp
            {
                tracing::warn!(
                    "OperatorsCache: cached operators are too old (cached: {}, current: {})",
                    cached.timestamp(),
                    current_slot_timestamp
                );
            }
        }

        let res = self
            .get_operators_for_current_and_next_epoch_internal(current_slot_timestamp)
            .await;

        match res {
            Ok(operators) => {
                let state = OperatorsCacheState::new(
                    current_slot_timestamp,
                    operators.current,
                    operators.next,
                );
                self.update_cache(state.clone());
                Ok(state)
            }
            Err(e) => {
                tracing::trace!(
                    "OperatorsCache: Error for slot timestamp {}: {}",
                    current_slot_timestamp,
                    e
                );
                self.read_cache_or_error(current_slot_timestamp)
            }
        }
    }

    fn read_cached_state(&self) -> Option<OperatorsCacheState> {
        match self.cache.read() {
            Ok(guard) => guard.clone(),
            Err(e) => {
                tracing::warn!("OperatorsCache: failed to read cache due to poisoned lock: {e}");
                None
            }
        }
    }

    fn update_cache(&self, state: OperatorsCacheState) {
        match self.cache.write() {
            Ok(mut guard) => {
                *guard = Some(state);
            }
            Err(e) => {
                tracing::warn!("OperatorsCache: failed to update cache due to poisoned lock: {e}");
            }
        }
    }

    fn read_cache_or_error(
        &self,
        current_slot_timestamp: u64,
    ) -> Result<OperatorsCacheState, Error> {
        self.read_cached_state().ok_or_else(|| {
            anyhow::anyhow!(
                "OperatorsCache: cache is empty, slot timestamp {}",
                current_slot_timestamp
            )
        })
    }

    async fn get_operators_for_current_and_next_epoch_internal(
        &self,
        current_slot_timestamp: u64,
    ) -> Result<Operators, OperatorsCacheError> {
        tracing::trace!(
            "OperatorsCache: for slot timestamp: {}",
            current_slot_timestamp
        );
        let block_header = self
            .provider
            .get_block(alloy::eips::BlockId::latest())
            .await
            .map_err(|e| OperatorsCacheError::LatestBlockFetchFailed {
                source: e.to_string(),
            })?
            .ok_or(OperatorsCacheError::LatestBlockNotFound)?
            .header;

        if block_header.timestamp < current_slot_timestamp {
            return Err(OperatorsCacheError::RpcBehindCurrentSlot {
                block_timestamp: block_header.timestamp,
            });
        }

        let whitelist = PreconfWhitelist::new(self.whitelist_address, &self.provider);
        let block_id = alloy::eips::BlockId::Hash(block_header.hash.into());
        let current_op = whitelist
            .getOperatorForCurrentEpoch()
            .block(block_id)
            .call()
            .await
            .map_err(|e| OperatorsCacheError::CurrentOperatorFetchFailed {
                source: e.to_string(),
            })?;

        let next_op = whitelist
            .getOperatorForNextEpoch()
            .block(block_id)
            .call()
            .await
            .map_err(|e| OperatorsCacheError::NextOperatorFetchFailed {
                source: e.to_string(),
            })?;

        Ok(Operators {
            current: current_op,
            next: next_op,
        })
    }
}
