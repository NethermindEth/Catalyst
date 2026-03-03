use std::collections::VecDeque;
use std::sync::Arc;

use anyhow::Error;
use common::{
    batch_builder::BatchBuilderConfig, l1::ethereum_l1::EthereumL1, shared::l2_block_v2::L2BlockV2,
};
use shasta::l1::execution_layer::ExecutionLayer as ShastaExecutionLayer;
use tracing::{debug, info};

struct Proposal {
    anchor_block_id: u64,
    l2_blocks: Vec<L2BlockV2>,
    total_bytes: u64,
}

pub struct ProposalManager {
    proposals_to_send: VecDeque<Proposal>,
    current_proposal: Option<Proposal>,
    config: BatchBuilderConfig,
    ethereum_l1: Arc<EthereumL1<ShastaExecutionLayer>>,
}

impl ProposalManager {
    pub fn new(
        config: BatchBuilderConfig,
        ethereum_l1: Arc<EthereumL1<ShastaExecutionLayer>>,
    ) -> Self {
        Self {
            proposals_to_send: VecDeque::new(),
            current_proposal: None,
            config,
            ethereum_l1,
        }
    }

    /// Returns the anchor block ID of the current proposal, if one exists.
    pub fn current_anchor_block_id(&self) -> Option<u64> {
        self.current_proposal.as_ref().map(|p| p.anchor_block_id)
    }

    /// Creates a new proposal with the given anchor block ID.
    /// Finalizes the current proposal first if one exists.
    pub fn create_proposal(&mut self, anchor_block_id: u64) {
        self.finalize_current_proposal();
        self.current_proposal = Some(Proposal {
            anchor_block_id,
            l2_blocks: Vec::new(),
            total_bytes: 0,
        });
        debug!(
            "Created new proposal with anchor_block_id={}",
            anchor_block_id
        );
    }

    /// Adds an L2 block to the current proposal.
    /// The block's `anchor_block_number` is set to the current proposal's anchor.
    /// Panics if there is no current proposal — caller must ensure one exists
    /// via `current_anchor_block_id()` / `create_proposal()`.
    pub fn add_l2_block(&mut self, l2_block: L2BlockV2) {
        let proposal = self
            .current_proposal
            .as_mut()
            .expect("add_l2_block called without a current proposal");

        proposal.total_bytes += l2_block.prebuilt_tx_list.bytes_length;
        proposal.l2_blocks.push(l2_block);

        debug!(
            "Added L2 block to proposal: anchor_block_id={}, blocks={}, total_bytes={}",
            proposal.anchor_block_id,
            proposal.l2_blocks.len(),
            proposal.total_bytes,
        );

        // Finalize if adding one more block would exceed the limit
        let block_count: u16 = proposal
            .l2_blocks
            .len()
            .try_into()
            .expect("block count should fit in u16");
        if !self.config.is_within_block_limit(block_count + 1) {
            info!("Proposal full ({} blocks), finalizing", block_count);
            self.finalize_current_proposal();
        }
    }

    pub async fn try_submit_oldest_proposal(&mut self) -> Result<(), Error> {
        let Some(proposal) = self.proposals_to_send.front() else {
            return Ok(());
        };

        if self
            .ethereum_l1
            .execution_layer
            .is_transaction_in_progress()
            .await?
        {
            debug!(
                "Cannot submit proposal, transaction is in progress. Queue: {}",
                self.proposals_to_send.len()
            );
            return Ok(());
        }

        info!(
            "Submitting proposal: anchor_block_id={}, blocks={}, total_bytes={}, queue_size={}",
            proposal.anchor_block_id,
            proposal.l2_blocks.len(),
            proposal.total_bytes,
            self.proposals_to_send.len(),
        );

        if let Err(err) = self
            .ethereum_l1
            .execution_layer
            .send_batch_to_l1(proposal.l2_blocks.clone(), 0)
            .await
        {
            self.proposals_to_send.clear();
            return Err(err);
        }

        self.proposals_to_send.pop_front();
        Ok(())
    }

    pub fn get_number_of_proposals(&self) -> u64 {
        self.proposals_to_send.len() as u64
            + if self.current_proposal.is_some() {
                1
            } else {
                0
            }
    }

    pub fn get_number_of_proposals_ready_to_send(&self) -> u64 {
        self.proposals_to_send.len() as u64
    }

    fn finalize_current_proposal(&mut self) {
        if let Some(proposal) = self.current_proposal.take()
            && !proposal.l2_blocks.is_empty()
        {
            self.proposals_to_send.push_back(proposal);
        }
    }
}
