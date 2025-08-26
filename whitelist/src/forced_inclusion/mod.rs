use crate::l1::execution_layer::ExecutionLayer;
use anyhow::Error;
use common::{
    blob::blob_parser::extract_transactions_from_blob,
    l1::{ethereum_l1::EthereumL1, forced_inclusion_info::ForcedInclusionInfo},
};
use std::sync::atomic::Ordering;
use std::sync::{Arc, atomic::AtomicU64};

pub struct ForcedInclusion {
    ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
    index: AtomicU64,
}

impl ForcedInclusion {
    pub fn new(ethereum_l1: Arc<EthereumL1<ExecutionLayer>>) -> Self {
        Self {
            ethereum_l1,
            index: AtomicU64::new(0),
        }
    }

    pub async fn sync_queue_index_with_head(&self) -> Result<u64, Error> {
        let head = self
            .ethereum_l1
            .execution_layer
            .extension
            .get_forced_inclusion_head()
            .await?;
        self.index.store(head, Ordering::SeqCst);
        tracing::debug!("sync_queue_index_with_head head: {}", head);
        Ok(head)
    }

    pub async fn decode_current_forced_inclusion(
        &self,
    ) -> Result<Option<ForcedInclusionInfo>, Error> {
        let i = self.index.load(Ordering::SeqCst);
        let tail = self
            .ethereum_l1
            .execution_layer
            .extension
            .get_forced_inclusion_tail()
            .await?;
        tracing::debug!("Decode forced inclusion at index {}, tail: {}", i, tail);
        if i >= tail {
            return Ok(None);
        }
        let forced_inclusion = self
            .ethereum_l1
            .execution_layer
            .extension
            .get_forced_inclusion(i)
            .await?;

        let txs = extract_transactions_from_blob(
            self.ethereum_l1.clone(),
            forced_inclusion.blobCreatedIn,
            [forced_inclusion.blobHash].to_vec(),
            forced_inclusion.blobByteOffset,
            forced_inclusion.blobByteSize,
        )
        .await?;

        Ok(Some(ForcedInclusionInfo {
            blob_hash: forced_inclusion.blobHash,
            blob_byte_offset: forced_inclusion.blobByteOffset,
            blob_byte_size: forced_inclusion.blobByteSize,
            created_in: forced_inclusion.blobCreatedIn,
            txs,
        }))
    }

    pub async fn consume_forced_inclusion(&self) -> Result<Option<ForcedInclusionInfo>, Error> {
        let fi = self.decode_current_forced_inclusion().await?;
        if fi.is_some() {
            self.increment_index();
        }
        Ok(fi)
    }

    pub fn increment_index(&self) {
        self.index.fetch_add(1, Ordering::SeqCst);
    }
}
