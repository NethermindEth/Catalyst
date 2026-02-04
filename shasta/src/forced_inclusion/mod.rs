use crate::l1::execution_layer::ExecutionLayer;
use alloy::rpc::types::Transaction;
use anyhow::Error;
use common::shared::l2_tx_lists::convert_tx_envelopes_to_transactions;
use common::{blob::blob_parser::get_bytes_from_blobs, l1::ethereum_l1::EthereumL1};
use std::sync::{Arc};

use taiko_protocol::shasta::manifest::DerivationSourceManifest;

pub struct ForcedInclusion {
    ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
    index: u64,
}

impl ForcedInclusion {
    pub async fn new(ethereum_l1: Arc<EthereumL1<ExecutionLayer>>) -> Result<Self, Error> {
        let index = ethereum_l1
            .execution_layer
            .get_forced_inclusion_head()
            .await?;
        Ok(Self {
            ethereum_l1,
            index,
        })
    }

    pub fn new_with_index(ethereum_l1: Arc<EthereumL1<ExecutionLayer>>, index: u64) -> Self {
        Self {
            ethereum_l1,
            index,
        }
    }

    pub fn set_index(&mut self, index: u64) {
        self.index = index;
    }

    pub async fn sync_queue_index_with_head(&mut self) -> Result<u64, Error> {
        let head = self
            .ethereum_l1
            .execution_layer
            .get_forced_inclusion_head()
            .await?;
        self.index = head;

        tracing::debug!("sync_queue_index_with_head head: {}", head);
        Ok(head)
    }

    pub async fn decode_current_forced_inclusion(&self) -> Result<Option<Vec<Transaction>>, Error> {
        let tail = self
            .ethereum_l1
            .execution_layer
            .get_forced_inclusion_tail()
            .await?;
        tracing::debug!("Decode forced inclusion at index {}, tail: {}", self.index, tail);
        if self.index >= tail {
            return Ok(None);
        }
        let forced_inclusion = self
            .ethereum_l1
            .execution_layer
            .get_forced_inclusion(self.index)
            .await?;

        let blob_bytes = get_bytes_from_blobs(
            self.ethereum_l1.clone(),
            forced_inclusion.blobSlice.timestamp.to::<u64>(),
            forced_inclusion.blobSlice.blobHashes,
        )
        .await?;

        let fi = DerivationSourceManifest::decompress_and_decode(
            &blob_bytes,
            forced_inclusion.blobSlice.offset.to::<usize>(),
        )?;

        // TODO handle multiple blocks in forced inclusion manifest
        if fi.blocks.len() != 1 {
            return Err(anyhow::anyhow!(
                "Expected exactly one block in forced inclusion manifest, found {}",
                fi.blocks.len()
            ));
        }
        if let Some(first_block) = fi.blocks.first() {
            Ok(Some(convert_tx_envelopes_to_transactions(
                first_block.transactions.clone(),
            )?))
        } else {
            Err(anyhow::anyhow!(
                "No blocks found in forced inclusion manifest"
            ))
        }
    }

    pub async fn consume_forced_inclusion(&mut self) -> Result<Option<Vec<Transaction>>, Error> {
        let start = std::time::Instant::now();
        let fi = self.decode_current_forced_inclusion().await?;
        if fi.is_some() {
            self.increment_index();
        }
        tracing::debug!(
            "Decoded forced inclusion in {} ms",
            start.elapsed().as_millis()
        );
        Ok(fi)
    }

    fn increment_index(&mut self) {
        self.index += 1;
    }

    pub async fn release_forced_inclusion(&mut self) {
        if self.index > 0 {
            self.index -= 1;
        } else {
            tracing::error!("Attempted to release forced inclusion index below zero");
        }
    }
}
