use crate::l1::bindings::ICheckpointStore::Checkpoint;
use crate::node::proposal_manager::{
    bridge_handler::{L1Call, UserOp},
    l2_block_payload::L2BlockV2Payload,
};
use alloy::primitives::{Address, B256, FixedBytes};
use common::shared::l2_block_v2::{L2BlockV2, L2BlockV2Draft};
use std::collections::VecDeque;
use std::time::Instant;
use taiko_protocol::shasta::manifest::{BlockManifest, DerivationSourceManifest};
use tracing::{debug, warn};

pub type Proposals = VecDeque<Proposal>;

#[derive(Default, Clone)]
pub struct Proposal {
    pub l2_blocks: Vec<L2BlockV2>,
    pub total_bytes: u64,
    pub coinbase: Address,

    // RealTime: maxAnchor instead of anchor
    pub max_anchor_block_number: u64,
    pub max_anchor_block_hash: B256,
    pub max_anchor_state_root: B256,

    // Proof fields
    pub checkpoint: Checkpoint,
    pub last_finalized_block_hash: B256,

    // Surge POC fields (carried over)
    pub user_ops: Vec<UserOp>,
    pub l2_user_op_ids: Vec<u64>,
    pub signal_slots: Vec<FixedBytes<32>>,
    pub l1_calls: Vec<L1Call>,

    // ZK proof (populated after Raiko call)
    pub zk_proof: Option<Vec<u8>>,
}

impl Proposal {
    pub fn compress(&mut self) {
        let start = Instant::now();

        let mut block_manifests = <Vec<BlockManifest>>::with_capacity(self.l2_blocks.len());
        for l2_block in &self.l2_blocks {
            block_manifests.push(BlockManifest {
                timestamp: l2_block.timestamp_sec,
                coinbase: l2_block.coinbase,
                anchor_block_number: l2_block.anchor_block_number,
                gas_limit: l2_block.gas_limit_without_anchor,
                transactions: l2_block
                    .prebuilt_tx_list
                    .tx_list
                    .iter()
                    .map(|tx| tx.clone().into())
                    .collect(),
            });
        }

        let manifest = DerivationSourceManifest {
            blocks: block_manifests,
        };

        let manifest_data = match manifest.encode_and_compress() {
            Ok(data) => data,
            Err(err) => {
                warn!("Failed to compress proposal manifest: {err}");
                return;
            }
        };

        debug!(
            "Proposal compression completed in {} ms. Total bytes before: {}. Total bytes after: {}.",
            start.elapsed().as_millis(),
            self.total_bytes,
            manifest_data.len()
        );

        self.total_bytes = manifest_data.len() as u64;
    }

    fn create_block_from_draft(&mut self, l2_draft_block: L2BlockV2Draft) -> L2BlockV2 {
        L2BlockV2::new_from(
            l2_draft_block.prebuilt_tx_list,
            l2_draft_block.timestamp_sec,
            self.coinbase,
            self.max_anchor_block_number,
            l2_draft_block.gas_limit_without_anchor,
        )
    }

    pub fn add_l2_block(&mut self, l2_block: L2BlockV2) -> L2BlockV2Payload {
        let l2_payload = L2BlockV2Payload {
            coinbase: self.coinbase,
            tx_list: l2_block.prebuilt_tx_list.tx_list.clone(),
            timestamp_sec: l2_block.timestamp_sec,
            gas_limit_without_anchor: l2_block.gas_limit_without_anchor,
            anchor_block_id: self.max_anchor_block_number,
            anchor_block_hash: self.max_anchor_block_hash,
            anchor_state_root: self.max_anchor_state_root,
        };
        self.total_bytes += l2_block.prebuilt_tx_list.bytes_length;
        self.l2_blocks.push(l2_block);
        l2_payload
    }

    pub fn add_l2_draft_block(&mut self, l2_draft_block: L2BlockV2Draft) -> L2BlockV2Payload {
        let l2_block = self.create_block_from_draft(l2_draft_block);
        self.add_l2_block(l2_block)
    }
}
