use std::{collections::VecDeque, sync::Arc};

use super::proposal::Proposals;
use crate::node::proposal_manager::l2_block_payload::L2BlockV2Payload;
use crate::{
    l1::execution_layer::ExecutionLayer, metrics::Metrics,
    node::proposal_manager::proposal::Proposal, shared::l2_tx_lists::PreBuiltTxList,
};
use alloy::primitives::Address;
use anyhow::Error;
use common::{
    batch_builder::BatchBuilderConfig,
    shared::l2_block_v2::{L2BlockV2, L2BlockV2Draft},
};
use common::{
    l1::{ethereum_l1::EthereumL1, slot_clock::SlotClock, transaction_error::TransactionError},
    shared::anchor_block_info::AnchorBlockInfo,
};
use taiko_bindings::anchor::ICheckpointStore::Checkpoint;
use tracing::{debug, trace, warn};

pub struct BatchBuilder {
    config: BatchBuilderConfig,
    proposals_to_send: VecDeque<Proposal>,
    current_proposal: Option<Proposal>,
    slot_clock: Arc<SlotClock>,
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

    // TODO use L2BlockV2 here
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
                // first compression, compressing the batch without the new L2 block
                batch.compress();
                new_total_bytes = batch.total_bytes + l2_draft_block.prebuilt_tx_list.bytes_length;
                if !self.config.is_within_bytes_limit(new_total_bytes) {
                    // second compression, compressing the batch with the new L2 block
                    // we can tolerate the processing overhead as it's a very rare case
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

    pub fn current_proposal_is_empty(&self) -> bool {
        self.current_proposal
            .as_ref()
            .is_none_or(|b| b.l2_blocks.is_empty())
    }

    pub fn create_new_batch(&mut self, id: u64, anchor_block: AnchorBlockInfo) {
        self.finalize_current_batch();

        self.current_proposal = Some(Proposal {
            id,
            l2_blocks: vec![],
            total_bytes: 0,
            coinbase: self.config.default_coinbase,
            anchor_block_id: anchor_block.id(),
            anchor_block_timestamp_sec: anchor_block.timestamp_sec(),
            anchor_block_hash: anchor_block.hash(),
            anchor_state_root: anchor_block.state_root(),
            num_forced_inclusion: 0,
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

    pub fn add_fi_block(
        &mut self,
        fi_block: L2BlockV2Draft,
        anchor_params: Checkpoint,
    ) -> Result<L2BlockV2Payload, Error> {
        if let Some(current_proposal) = self.current_proposal.as_mut() {
            let payload = current_proposal.add_forced_inclusion(fi_block, anchor_params);

            debug!(
                "Added forced inclusion L2 draft block to batch: l2 blocks: {}, total bytes: {}",
                current_proposal.l2_blocks.len(),
                current_proposal.total_bytes
            );
            Ok(payload)
        } else {
            Err(anyhow::anyhow!("No current batch"))
        }
    }

    pub fn add_l2_block_and_get_current_proposal(
        &mut self,
        l2_block: L2BlockV2,
    ) -> Result<&Proposal, Error> {
        if let Some(current_proposal) = self.current_proposal.as_mut() {
            current_proposal.add_l2_block(l2_block);

            debug!(
                "Added L2 block to batch: l2 blocks: {}, total bytes: {}",
                current_proposal.l2_blocks.len(),
                current_proposal.total_bytes
            );
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

    #[allow(clippy::too_many_arguments)]
    pub async fn recover_from(
        &mut self,
        proposal_id: u64,
        anchor_info: AnchorBlockInfo,
        coinbase: Address,
        tx_list: Vec<alloy::rpc::types::Transaction>,
        l2_block_timestamp_sec: u64,
        gas_limit: u64,
        is_forced_inclusion: bool,
    ) -> Result<(), Error> {
        // We have a new proposal when proposal ID differs
        // Otherwise we continue with the current proposal
        if !self.is_same_proposal_id(proposal_id) {
            self.finalize_current_batch();
            debug!(
                "Creating new proposal during recovery: proposal_id {}, anchor_block_id {} coinbase {}",
                proposal_id,
                anchor_info.id(),
                coinbase
            );
            self.current_proposal = Some(Proposal {
                id: proposal_id,
                total_bytes: 0,
                l2_blocks: vec![],
                coinbase,
                anchor_block_id: anchor_info.id(),
                anchor_block_timestamp_sec: anchor_info.timestamp_sec(),
                anchor_block_hash: anchor_info.hash(),
                anchor_state_root: anchor_info.state_root(),
                num_forced_inclusion: 0,
            });
        }

        if is_forced_inclusion {
            if let Some(batch) = self.current_proposal.as_ref()
                && !batch.l2_blocks.is_empty()
            {
                return Err(anyhow::anyhow!(
                    "recover_from: Cannot add forced inclusion L2 block to non-empty proposal"
                ));
            }

            self.inc_forced_inclusion()?;
        } else {
            if let Some(batch) = self.current_proposal.as_mut()
                && batch.anchor_block_id < anchor_info.id()
            {
                batch.anchor_block_id = anchor_info.id();
                batch.anchor_block_timestamp_sec = anchor_info.timestamp_sec();
                batch.anchor_block_hash = anchor_info.hash();
                batch.anchor_state_root = anchor_info.state_root();
            }

            let bytes_length =
                crate::shared::l2_tx_lists::encode_and_compress(&tx_list)?.len() as u64;

            let l2_block = L2BlockV2::new_from(
                crate::shared::l2_tx_lists::PreBuiltTxList {
                    tx_list,
                    estimated_gas_used: 0,
                    bytes_length,
                },
                l2_block_timestamp_sec,
                coinbase,
                anchor_info.id(),
                gas_limit,
            );

            // TODO we add block to the current proposal.
            // But we should verify that it fit N blob data size
            // Otherwise we should do a reorg
            // TODO align on blob count with all teams

            // at previous step we check that proposal exists
            self.add_l2_block_and_get_current_proposal(l2_block)?;
        }
        Ok(())
    }

    fn is_same_proposal_id(&self, proposal_id: u64) -> bool {
        // Note: proposal.id is not part of BatchLike trait, so we need to access it directly
        // Since Proposal has a public id field, we can access it
        self.current_proposal
            .as_ref()
            .is_some_and(|proposal| proposal.id == proposal_id)
    }

    pub fn is_empty(&self) -> bool {
        trace!(
            "batch_builder::is_empty: current_proposal is none: {}, proposals_to_send len: {}",
            self.current_proposal.is_none(),
            self.proposals_to_send.len()
        );
        self.current_proposal.is_none() && self.proposals_to_send.is_empty()
    }

    pub async fn try_submit_oldest_batch(
        &mut self,
        ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
        submit_only_full_batches: bool,
    ) -> Result<(), Error> {
        if self.current_proposal.is_some()
            && (!submit_only_full_batches
                || !self.config.is_within_block_limit(
                    u16::try_from(
                        self.current_proposal
                            .as_ref()
                            .map(|b| b.l2_blocks.len())
                            .unwrap_or(0),
                    )? + 1,
                ))
        {
            self.finalize_current_batch();
        }

        if let Some(batch) = self.proposals_to_send.front() {
            if ethereum_l1
                .execution_layer
                .transaction_monitor
                .is_transaction_in_progress()
                .await?
            {
                debug!(
                    proposals_to_send = %self.proposals_to_send.len(),
                    current_proposal = %self.current_proposal.is_some(),
                    "Cannot submit batch, transaction is in progress.",
                );
                return Err(anyhow::anyhow!(
                    "Cannot submit batch, transaction is in progress."
                ));
            }

            debug!(
                anchor_block_id = %batch.anchor_block_id,
                coinbase = %batch.coinbase,
                l2_blocks_len = %batch.l2_blocks.len(),
                total_bytes = %batch.total_bytes,
                proposals_to_send = %self.proposals_to_send.len(),
                current_proposal = %self.current_proposal.is_some(),
                "Submitting batch"
            );

            if let Err(err) = ethereum_l1
                .execution_layer
                // TODO send a Proosal to function
                .send_batch_to_l1(batch.l2_blocks.clone(), batch.num_forced_inclusion)
                .await
            {
                if let Some(transaction_error) = err.downcast_ref::<TransactionError>()
                    && !matches!(transaction_error, TransactionError::EstimationTooEarly)
                {
                    debug!("BatchBuilder: Transaction error, removing all batches");
                    self.proposals_to_send.clear();
                }
                return Err(err);
            }

            self.proposals_to_send.pop_front();
        }

        Ok(())
    }

    // TODO do we have that check in SC?
    pub fn is_time_shift_expired(&self, current_l2_slot_timestamp: u64) -> bool {
        if let Some(current_proposal) = self.current_proposal.as_ref()
            && let Some(last_block) = current_proposal.l2_blocks.last()
        {
            return current_l2_slot_timestamp - last_block.timestamp_sec
                > self.config.max_time_shift_between_blocks_sec;
        }
        false
    }
    // TODO do we have that check in SC?
    pub fn is_time_shift_between_blocks_expiring(&self, current_l2_slot_timestamp: u64) -> bool {
        if let Some(current_proposal) = self.current_proposal.as_ref() {
            // l1_batches is not empty
            if let Some(last_block) = current_proposal.l2_blocks.last() {
                if current_l2_slot_timestamp < last_block.timestamp_sec {
                    warn!("Preconfirmation timestamp is before the last block timestamp");
                    return false;
                }
                // is the last L1 slot to add an empty L2 block so we don't have a time shift overflow
                return self.is_the_last_l1_slot_to_add_an_empty_l2_block(
                    current_l2_slot_timestamp,
                    last_block.timestamp_sec,
                );
            }
        }
        false
    }
    // TODO do we have that check in SC?
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
            let slots_since_l1_block = self
                .slot_clock
                .slots_since_l1_block(current_proposal.anchor_block_timestamp_sec)?;
            return Ok(slots_since_l1_block > self.config.max_anchor_height_offset);
        }
        Ok(false)
    }

    fn is_empty_block_required(&self, preconfirmation_timestamp: u64) -> bool {
        self.is_time_shift_between_blocks_expiring(preconfirmation_timestamp)
    }

    pub fn clone_without_batches(&self) -> Self {
        Self {
            config: self.config.clone(),
            proposals_to_send: VecDeque::new(),
            current_proposal: None,
            slot_clock: self.slot_clock.clone(),
            metrics: self.metrics.clone(),
        }
    }

    pub fn get_number_of_batches(&self) -> u64 {
        self.proposals_to_send.len() as u64
            + if self.current_proposal.is_some() {
                1
            } else {
                0
            }
    }

    /// Alias for `take_proposals_to_send` for compatibility
    pub fn take_batches_to_send(&mut self) -> VecDeque<Proposal> {
        std::mem::take(&mut self.proposals_to_send)
    }

    pub fn prepend_batches(&mut self, mut batches: Proposals) {
        batches.append(&mut self.proposals_to_send);
        self.proposals_to_send = batches;
    }

    pub fn get_current_proposal_id(&self) -> Option<u64> {
        self.current_proposal.as_ref().map(|b| b.id)
    }

    pub fn try_finalize_current_batch(&mut self) -> Result<(), Error> {
        // TODO handle forced inclusion
        self.finalize_current_batch();
        Ok(())
    }

    pub fn remove_current_batch(&mut self) {
        self.current_proposal = None;
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

    pub fn has_current_forced_inclusion(&self) -> bool {
        self.current_proposal
            .as_ref()
            .map(|p| p.num_forced_inclusion > 0)
            .unwrap_or(false)
    }

    pub fn inc_forced_inclusion(&mut self) -> Result<(), Error> {
        self.current_proposal
            .as_mut()
            .map(|proposal| proposal.num_forced_inclusion += 1)
            .ok_or_else(|| anyhow::anyhow!("No current proposal to add forced inclusion to"))
    }
}
