use crate::l1::bindings::ICheckpointStore::Checkpoint;
use crate::node::proposal_manager::{
    bridge_handler::{L1Call, UserOp},
    l2_block_payload::L2BlockV2Payload,
    proposal::Proposal,
};
use alloy::primitives::{B256, FixedBytes};
use anyhow::Error;
use common::metrics::Metrics;
use common::{
    batch_builder::BatchBuilderConfig,
    shared::l2_block_v2::{L2BlockV2, L2BlockV2Draft},
};
use common::{l1::slot_clock::SlotClock, shared::anchor_block_info::AnchorBlockInfo};
use std::{collections::VecDeque, sync::Arc};
use tracing::{debug, info, trace, warn};

pub struct BatchBuilder {
    config: BatchBuilderConfig,
    proposals_to_send: VecDeque<Proposal>,
    current_proposal: Option<Proposal>,
    slot_clock: Arc<SlotClock>,
    #[allow(dead_code)]
    metrics: Arc<Metrics>,
}

impl BatchBuilder {
    pub fn new(
        config: BatchBuilderConfig,
        slot_clock: Arc<SlotClock>,
        metrics: Arc<Metrics>,
    ) -> Self {
        Self {
            config,
            proposals_to_send: VecDeque::new(),
            current_proposal: None,
            slot_clock,
            metrics,
        }
    }

    pub fn get_config(&self) -> &BatchBuilderConfig {
        &self.config
    }

    pub fn can_consume_l2_block(&mut self, l2_draft_block: &L2BlockV2Draft) -> bool {
        let is_time_shift_expired = self.is_time_shift_expired(l2_draft_block.timestamp_sec);
        self.current_proposal.as_mut().is_some_and(|batch| {
            let new_block_count = match u16::try_from(batch.l2_blocks.len() + 1) {
                Ok(n) => n,
                Err(_) => return false,
            };

            let mut new_total_bytes =
                batch.total_bytes + l2_draft_block.prebuilt_tx_list.bytes_length;

            if !self.config.is_within_bytes_limit(new_total_bytes) {
                batch.compress();
                new_total_bytes = batch.total_bytes + l2_draft_block.prebuilt_tx_list.bytes_length;
                if !self.config.is_within_bytes_limit(new_total_bytes) {
                    let start = std::time::Instant::now();
                    let mut batch_clone = batch.clone();
                    batch_clone.add_l2_draft_block(l2_draft_block.clone());
                    batch_clone.compress();
                    new_total_bytes = batch_clone.total_bytes;
                    debug!(
                        "can_consume_l2_block: Second compression took {} ms, new total bytes: {}",
                        start.elapsed().as_millis(),
                        new_total_bytes
                    );
                }
            }

            self.config.is_within_bytes_limit(new_total_bytes)
                && self.config.is_within_block_limit(new_block_count)
                && !is_time_shift_expired
        })
    }

    pub fn create_new_batch(
        &mut self,
        anchor_block: AnchorBlockInfo,
        last_finalized_block_hash: B256,
    ) {
        self.finalize_current_batch();

        self.current_proposal = Some(Proposal {
            l2_blocks: vec![],
            total_bytes: 0,
            coinbase: self.config.default_coinbase,
            max_anchor_block_number: anchor_block.id(),
            max_anchor_block_hash: anchor_block.hash(),
            max_anchor_state_root: anchor_block.state_root(),
            checkpoint: Checkpoint::default(),
            last_finalized_block_hash,
            user_ops: vec![],
            l2_user_op_ids: vec![],
            signal_slots: vec![],
            l1_calls: vec![],
            zk_proof: None,
        });
    }

    pub fn add_l2_draft_block(
        &mut self,
        l2_draft_block: L2BlockV2Draft,
    ) -> Result<L2BlockV2Payload, Error> {
        if let Some(current_proposal) = self.current_proposal.as_mut() {
            let payload = current_proposal.add_l2_draft_block(l2_draft_block);

            debug!(
                "Added L2 draft block to batch: l2 blocks: {}, total bytes: {}",
                current_proposal.l2_blocks.len(),
                current_proposal.total_bytes
            );
            Ok(payload)
        } else {
            Err(anyhow::anyhow!("No current batch"))
        }
    }

    /// Add a pre-built L2BlockV2 directly to the current proposal.
    /// Used during recovery to bypass the draft/payload flow.
    #[allow(dead_code)]
    pub fn add_recovered_l2_block(&mut self, l2_block: L2BlockV2) -> Result<(), Error> {
        if let Some(current_proposal) = self.current_proposal.as_mut() {
            current_proposal.total_bytes += l2_block.prebuilt_tx_list.bytes_length;
            current_proposal.l2_blocks.push(l2_block);
            Ok(())
        } else {
            Err(anyhow::anyhow!("No current batch for recovered block"))
        }
    }

    pub fn add_user_op(&mut self, user_op_data: UserOp) -> Result<&Proposal, Error> {
        if let Some(current_proposal) = self.current_proposal.as_mut() {
            current_proposal.user_ops.push(user_op_data.clone());
            info!("Added user op: {:?}", user_op_data);
            Ok(current_proposal)
        } else {
            Err(anyhow::anyhow!("No current batch"))
        }
    }

    pub fn add_l2_user_op_id(&mut self, id: u64) -> Result<(), Error> {
        if let Some(current_proposal) = self.current_proposal.as_mut() {
            current_proposal.l2_user_op_ids.push(id);
            Ok(())
        } else {
            Err(anyhow::anyhow!("No current batch for L2 user op id"))
        }
    }

    pub fn add_signal_slot(&mut self, signal_slot: FixedBytes<32>) -> Result<&Proposal, Error> {
        if let Some(current_proposal) = self.current_proposal.as_mut() {
            current_proposal.signal_slots.push(signal_slot);
            info!("Added signal slot: {:?}", signal_slot);
            Ok(current_proposal)
        } else {
            Err(anyhow::anyhow!("No current batch"))
        }
    }

    pub fn add_l1_call(&mut self, l1_call: L1Call) -> Result<&Proposal, Error> {
        if let Some(current_proposal) = self.current_proposal.as_mut() {
            current_proposal.l1_calls.push(l1_call.clone());
            info!("Added L1 call: {:?}", l1_call);
            Ok(current_proposal)
        } else {
            Err(anyhow::anyhow!("No current batch"))
        }
    }

    pub fn set_proposal_checkpoint(&mut self, checkpoint: Checkpoint) -> Result<&Proposal, Error> {
        if let Some(current_proposal) = self.current_proposal.as_mut() {
            current_proposal.checkpoint = checkpoint.clone();
            debug!("Update proposal checkpoint: {:?}", checkpoint);
            Ok(current_proposal)
        } else {
            Err(anyhow::anyhow!("No current batch"))
        }
    }

    pub fn get_current_proposal_last_block_timestamp(&self) -> Option<u64> {
        self.current_proposal
            .as_ref()
            .and_then(|p| p.l2_blocks.last().map(|b| b.timestamp_sec))
    }

    pub fn remove_last_l2_block(&mut self) {
        if let Some(current_proposal) = self.current_proposal.as_mut() {
            let removed_block = current_proposal.l2_blocks.pop();
            if let Some(removed_block) = removed_block {
                current_proposal.total_bytes -= removed_block.prebuilt_tx_list.bytes_length;
                if current_proposal.l2_blocks.is_empty() {
                    self.current_proposal = None;
                }
                debug!(
                    "Removed L2 block from batch: {} txs, {} bytes",
                    removed_block.prebuilt_tx_list.tx_list.len(),
                    removed_block.prebuilt_tx_list.bytes_length
                );
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        trace!(
            "batch_builder::is_empty: current_proposal is none: {}, proposals_to_send len: {}",
            self.current_proposal.is_none(),
            self.proposals_to_send.len()
        );
        self.current_proposal.is_none() && self.proposals_to_send.is_empty()
    }

    /// Finalize the current batch if appropriate for submission.
    pub fn finalize_if_needed(&mut self, submit_only_full_batches: bool) {
        if self.current_proposal.is_some()
            && (!submit_only_full_batches
                || !self.config.is_within_block_limit(
                    u16::try_from(
                        self.current_proposal
                            .as_ref()
                            .map(|b| b.l2_blocks.len())
                            .unwrap_or(0),
                    )
                    .unwrap_or(u16::MAX)
                        + 1,
                ))
        {
            self.finalize_current_batch();
        }
    }

    /// Pop the oldest finalized batch, stamping it with the current last_finalized_block_hash.
    pub fn pop_oldest_batch(&mut self, last_finalized_block_hash: B256) -> Option<Proposal> {
        if let Some(mut batch) = self.proposals_to_send.pop_front() {
            batch.last_finalized_block_hash = last_finalized_block_hash;
            Some(batch)
        } else {
            None
        }
    }

    /// Re-queue a batch at the front (e.g., when submission couldn't start).
    pub fn push_front_batch(&mut self, batch: Proposal) {
        self.proposals_to_send.push_front(batch);
    }

    pub fn is_time_shift_expired(&self, current_l2_slot_timestamp: u64) -> bool {
        if let Some(current_proposal) = self.current_proposal.as_ref()
            && let Some(last_block) = current_proposal.l2_blocks.last()
        {
            return current_l2_slot_timestamp - last_block.timestamp_sec
                > self.config.max_time_shift_between_blocks_sec;
        }
        false
    }

    pub fn is_time_shift_between_blocks_expiring(&self, current_l2_slot_timestamp: u64) -> bool {
        if let Some(current_proposal) = self.current_proposal.as_ref()
            && let Some(last_block) = current_proposal.l2_blocks.last()
        {
            if current_l2_slot_timestamp < last_block.timestamp_sec {
                warn!("Preconfirmation timestamp is before the last block timestamp");
                return false;
            }
            return self.is_the_last_l1_slot_to_add_an_empty_l2_block(
                current_l2_slot_timestamp,
                last_block.timestamp_sec,
            );
        }
        false
    }

    fn is_the_last_l1_slot_to_add_an_empty_l2_block(
        &self,
        current_l2_slot_timestamp: u64,
        last_block_timestamp: u64,
    ) -> bool {
        current_l2_slot_timestamp - last_block_timestamp
            >= self.config.max_time_shift_between_blocks_sec - self.config.l1_slot_duration_sec
    }

    pub fn is_greater_than_max_anchor_height_offset(&self) -> Result<bool, Error> {
        if let Some(current_proposal) = self.current_proposal.as_ref() {
            let current_l1_block = self.slot_clock.get_current_slot()?;
            if current_l1_block > current_proposal.max_anchor_block_number {
                let offset = current_l1_block - current_proposal.max_anchor_block_number;
                return Ok(offset > self.config.max_anchor_height_offset);
            }
        }
        Ok(false)
    }

    fn is_empty_block_required(&self, preconfirmation_timestamp: u64) -> bool {
        self.is_time_shift_between_blocks_expiring(preconfirmation_timestamp)
    }

    pub fn get_number_of_batches(&self) -> u64 {
        self.proposals_to_send.len() as u64
            + if self.current_proposal.is_some() {
                1
            } else {
                0
            }
    }

    pub fn finalize_current_batch(&mut self) {
        if let Some(batch) = self.current_proposal.take()
            && !batch.l2_blocks.is_empty()
        {
            self.proposals_to_send.push_back(batch);
        }
    }

    pub fn should_new_block_be_created(
        &self,
        pending_tx_list: &Option<PreBuiltTxList>,
        current_l2_slot_timestamp: u64,
        end_of_sequencing: bool,
    ) -> bool {
        let number_of_pending_txs = pending_tx_list
            .as_ref()
            .map(|tx_list| tx_list.tx_list.len())
            .unwrap_or(0) as u64;

        if self.is_empty_block_required(current_l2_slot_timestamp) || end_of_sequencing {
            return true;
        }

        if number_of_pending_txs >= self.config.preconf_min_txs {
            return true;
        }

        if let Some(current_proposal) = self.current_proposal.as_ref()
            && let Some(last_block) = current_proposal.l2_blocks.last()
        {
            let number_of_l2_slots =
                (current_l2_slot_timestamp.saturating_sub(last_block.timestamp_sec)) * 1000
                    / self.slot_clock.get_preconf_heartbeat_ms();
            return number_of_l2_slots > self.config.preconf_max_skipped_l2_slots;
        }

        true
    }
}

use common::shared::l2_tx_lists::PreBuiltTxList;
