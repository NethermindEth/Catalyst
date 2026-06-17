use crate::l1::bindings::PreconfWhitelist;
use alloy::{
    primitives::Address,
    providers::{DynProvider, Provider},
};
use anyhow::Error;
use std::sync::RwLock;

#[derive(Clone, Debug)]
struct Operators {
    current: Address,
    next: Address,
}

#[derive(Clone, Debug)]
pub struct OperatorsCacheState {
    timestamp: u64,
    operators: Operators,
}

impl OperatorsCacheState {
    pub fn new(timestamp: u64, current: Address, next: Address) -> Self {
        Self {
            timestamp,
            operators: Operators { current, next },
        }
    }

    pub fn timestamp(&self) -> u64 {
        self.timestamp
    }

    pub fn current_operator(&self) -> Address {
        self.operators.current
    }

    pub fn next_operator(&self) -> Address {
        self.operators.next
    }
}

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
            if cached.timestamp == current_slot_timestamp {
                return Ok(cached);
            } else if cached.timestamp.saturating_add(MAX_BLOCK_AGE_SECS) < current_slot_timestamp {
                tracing::warn!(
                    "OperatorsCache: cached operators are too old (cached: {}, current: {})",
                    cached.timestamp,
                    current_slot_timestamp
                );
            }
        }

        let res = self
            .get_operators_for_current_and_next_epoch_internal(current_slot_timestamp)
            .await;

        match res {
            Ok(Some(operators)) => {
                let state = OperatorsCacheState {
                    timestamp: current_slot_timestamp,
                    operators,
                };
                self.update_cache(state.clone());
                Ok(state)
            }
            Ok(None) => self.read_cache_or_error(current_slot_timestamp),
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
    ) -> Result<Option<Operators>, Error> {
        tracing::trace!(
            "OperatorsCache: for slot timestamp: {}",
            current_slot_timestamp
        );
        let block_header = self
            .provider
            .get_block(alloy::eips::BlockId::latest())
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to get latest block for slot timestamp {}: {}",
                    current_slot_timestamp,
                    e
                )
            })?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "No latest block found for slot timestamp {}",
                    current_slot_timestamp
                )
            })?
            .header;

        if block_header.timestamp < current_slot_timestamp {
            tracing::trace!(
                "OperatorsCache: RPC behind current slot (latest: {}, current: {}), not caching operator data",
                block_header.timestamp,
                current_slot_timestamp
            );
            return Ok(None);
        }

        let whitelist = PreconfWhitelist::new(self.whitelist_address, &self.provider);
        let block_id = alloy::eips::BlockId::Hash(block_header.hash.into());
        let current_op = whitelist
            .getOperatorForCurrentEpoch()
            .block(block_id)
            .call()
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to get current operator for slot timestamp {}: {}",
                    current_slot_timestamp,
                    e
                )
            })?;

        let next_op = whitelist
            .getOperatorForNextEpoch()
            .block(block_id)
            .call()
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to get next operator for slot timestamp {}: {}",
                    current_slot_timestamp,
                    e
                )
            })?;

        Ok(Some(Operators {
            current: current_op,
            next: next_op,
        }))
    }
}
