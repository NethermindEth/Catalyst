use crate::{l1::execution_layer::ExecutionLayer, l2::taiko::Taiko};
use anyhow::Error;
use common::l1::ethereum_l1::EthereumL1;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;

mod binary_search_last_block;
use binary_search_last_block::{ProposalIdFetcher, binary_search_last_block};

struct FindBlockResult {
    pub block_id: u64,
    pub safe_proposal_id: u64,
}

pub struct LastSafeL2BlockFinder {
    ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
    taiko: Arc<Taiko>,
    cache: RwLock<HashMap<u64, u64>>, // proposal_id -> last_block_id
}

impl ProposalIdFetcher for LastSafeL2BlockFinder {
    async fn get_proposal_id(&self, block_id: u64) -> Result<u64, Error> {
        self.taiko
            .l2_execution_layer()
            .get_proposal_id_from_geth_by_block_id(block_id)
            .await
    }
}

impl LastSafeL2BlockFinder {
    pub fn new(ethereum_l1: Arc<EthereumL1<ExecutionLayer>>, taiko: Arc<Taiko>) -> Self {
        Self {
            ethereum_l1,
            taiko,
            cache: RwLock::new(HashMap::new()),
        }
    }

    pub async fn get(&self) -> Result<u64, Error> {
        let inbox_state = self.ethereum_l1.execution_layer.get_inbox_state().await?;
        if inbox_state.nextProposalId == 1 {
            self.taiko.l2_execution_layer().get_head_l1_origin().await.or_else(|_| {
                tracing::warn!("LastSafeL2Block::get(): Failed to get L2 head from get_head_l1_origin, but nextProposalId is 1, so returning L2 height as 0");
                Ok(0u64)
            })
        } else {
            let target_proposal_id = inbox_state.nextProposalId.to::<u64>() - 1;
            tracing::debug!(
                "LastSafeL2Block::get(): Fetching L2 height from L1 nextProposalId: {}, target: {}",
                inbox_state.nextProposalId,
                target_proposal_id
            );

            // Check cache
            {
                let cache = self.cache.read().await;
                if let Some(&block_id) = cache.get(&target_proposal_id) {
                    tracing::debug!(
                        "LastSafeL2Block::get(): Cache hit for proposal_id {}: block_id {}",
                        target_proposal_id,
                        block_id
                    );
                    return Ok(block_id);
                }
            }

            let result = self
                .find_last_block_for_proposal_id(target_proposal_id)
                .await?;

            // Clear cache entries that are now outdated (below the safe proposal id)
            self.clear_cache(result.safe_proposal_id).await?;
            // Store result in cache
            let mut cache = self.cache.write().await;
            cache.insert(target_proposal_id, result.block_id);
            tracing::debug!(
                "LastSafeL2Block::get(): Cached block {} for proposal_id {}",
                result.block_id,
                target_proposal_id
            );

            Ok(result.block_id)
        }
    }

    async fn clear_cache(&self, safe_proposal_id: u64) -> Result<(), Error> {
        let mut cache = self.cache.write().await;
        let removed_count = cache.len();
        cache.retain(|&key, _| key >= safe_proposal_id);
        tracing::debug!(
            "LastSafeL2Block::clear_cache(): Removed {} entries below proposal_id {}",
            removed_count - cache.len(),
            safe_proposal_id
        );
        Ok(())
    }

    async fn find_last_block_for_proposal_id(
        &self,
        target_proposal_id: u64,
    ) -> Result<FindBlockResult, Error> {
        // Fast path: try direct lookup via Taiko
        match self
            .taiko
            .get_last_block_id_by_batch_id(target_proposal_id)
            .await
        {
            Ok(block_id) => {
                return Ok(FindBlockResult {
                    block_id,
                    safe_proposal_id: target_proposal_id,
                });
            }
            Err(err) => {
                tracing::warn!(
                    "LastSafeL2Block::find_last_block_for_proposal_id(): Failed to get last block id by batch id: {}",
                    err
                );
            }
        }

        // Slow path: search with geth calls
        let last_known_safe_block_id = self.taiko.l2_execution_layer().get_head_l1_origin().await?;
        let last_known_safe_proposal_id = self
            .taiko
            .l2_execution_layer()
            .get_proposal_id_from_geth_by_block_id(last_known_safe_block_id)
            .await?;

        let (last_block_id, last_proposal_id) = self
            .taiko
            .l2_execution_layer()
            .get_latest_block_id_and_proposal_id()
            .await?;
        tracing::debug!(
            "LastSafeL2Block::find_last_block_for_proposal_id(): Last known safe block: {}, known safe proposal id: {}, target proposal id: {}, last block id: {}, last proposal id: {}",
            last_known_safe_block_id,
            last_known_safe_proposal_id,
            target_proposal_id,
            last_block_id,
            last_proposal_id
        );

        if last_proposal_id == target_proposal_id {
            tracing::debug!(
                "LastSafeL2Block::find_last_block_for_proposal_id(): Last block {} has the target proposal_id {}, returning it",
                last_block_id,
                target_proposal_id
            );
            return Ok(FindBlockResult {
                block_id: last_block_id,
                safe_proposal_id: last_known_safe_proposal_id,
            });
        }

        if target_proposal_id < last_known_safe_proposal_id || target_proposal_id > last_proposal_id
        {
            return Err(anyhow::anyhow!(
                "LastSafeL2Block::find_last_block_for_proposal_id(): Target proposal id {} is out of range ({} - {})",
                target_proposal_id,
                last_known_safe_proposal_id,
                last_proposal_id
            ));
        }

        // Binary search for the last block with target_proposal_id or last block of previous proposal_id
        let result = binary_search_last_block(
            self,
            last_known_safe_block_id,
            last_block_id,
            target_proposal_id,
        )
        .await?;

        if let Some(block_id) = result {
            tracing::debug!(
                "LastSafeL2Block::find_last_block_for_proposal_id(): Found last block {} for proposal_id {}",
                block_id,
                target_proposal_id
            );
            return Ok(FindBlockResult {
                block_id,
                safe_proposal_id: last_known_safe_proposal_id,
            });
        }

        Err(anyhow::anyhow!(
            "LastSafeL2Block::find_last_block_for_proposal_id(): Failed to find block for proposal id {} in range {} - {}",
            target_proposal_id,
            last_known_safe_block_id,
            last_block_id
        ))
    }
}
