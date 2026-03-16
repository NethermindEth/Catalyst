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
    l1::{ethereum_l1::EthereumL1, slot_clock::SlotClock},
    shared::anchor_block_info::AnchorBlockInfo,
};
use taiko_bindings::anchor::ICheckpointStore::Checkpoint;
use tracing::{debug, trace, warn};

pub struct ProposalBuilder {
    config: BatchBuilderConfig,
    proposals_to_send: VecDeque<Proposal>,
    current_proposal: Option<Proposal>,
    slot_clock: Arc<SlotClock>,
    metrics: Arc<Metrics>,
}

impl ProposalBuilder {
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
        self.current_proposal.as_mut().is_some_and(|proposal| {
            let new_block_count = match u16::try_from(proposal.l2_blocks.len() + 1) {
                Ok(n) => n,
                Err(_) => return false,
            };

            let mut new_total_bytes =
                proposal.total_bytes + l2_draft_block.prebuilt_tx_list.bytes_length;

            if !self.config.is_within_bytes_limit(new_total_bytes) {
                // first compression, compressing the proposal without the new L2 block
                proposal.compress();
                new_total_bytes =
                    proposal.total_bytes + l2_draft_block.prebuilt_tx_list.bytes_length;
                if !self.config.is_within_bytes_limit(new_total_bytes) {
                    // second compression, compressing the proposal with the new L2 block
                    // we can tolerate the processing overhead as it's a very rare case
                    let start = std::time::Instant::now();
                    let mut proposal_clone = proposal.clone();
                    proposal_clone.add_l2_draft_block(l2_draft_block.clone());
                    proposal_clone.compress();
                    new_total_bytes = proposal_clone.total_bytes;
                    debug!(
                        "can_consume_l2_block: Second compression took {} ms, new total bytes: {}",
                        start.elapsed().as_millis(),
                        new_total_bytes
                    );
                }
            }

            self.config.is_within_bytes_limit(new_total_bytes)
                && self.config.is_within_block_limit(new_block_count)
        })
    }

    /// Returns true if the current proposal exists, has no common block and
    /// can accept more forced inclusion blocks.
    pub fn can_add_forced_inclusion(&self) -> bool {
        self.current_proposal.as_ref().is_some_and(|p| {
            p.l2_blocks.is_empty()
                && p.num_forced_inclusion
                    < taiko_protocol::shasta::constants::MAX_FORCED_INCLUSIONS_PER_PROPOSAL
        })
    }

    pub fn create_new_proposal(&mut self, id: u64, anchor_block: AnchorBlockInfo, timestamp: u64) {
        self.finalize_current_proposal();

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
            created_at_sec: timestamp,
            pending_confirmation: false,
        });
    }

    pub fn add_l2_draft_block(
        &mut self,
        l2_draft_block: L2BlockV2Draft,
    ) -> Result<L2BlockV2Payload, Error> {
        if let Some(current_proposal) = self.current_proposal.as_mut() {
            let payload = current_proposal.add_l2_draft_block(l2_draft_block);

            debug!(
                "Added L2 draft block to proposal: forced inclusions {}, l2 blocks: {}, total bytes: {}",
                current_proposal.num_forced_inclusion,
                current_proposal.l2_blocks.len(),
                current_proposal.total_bytes
            );
            Ok(payload)
        } else {
            Err(anyhow::anyhow!("No current proposal"))
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
                "Added forced inclusion L2 block to proposal: forced inclusions: {}, l2 blocks: {}, total bytes: {}",
                current_proposal.num_forced_inclusion,
                current_proposal.l2_blocks.len(),
                current_proposal.total_bytes
            );
            Ok(payload)
        } else {
            Err(anyhow::anyhow!("No current proposal"))
        }
    }

    pub fn add_l2_block_and_get_current_proposal(
        &mut self,
        l2_block: L2BlockV2,
    ) -> Result<&Proposal, Error> {
        if let Some(current_proposal) = self.current_proposal.as_mut() {
            current_proposal.add_l2_block(l2_block);

            debug!(
                "Added L2 block to proposal: forced inclusions {}, l2 blocks: {}, total bytes: {}",
                current_proposal.num_forced_inclusion,
                current_proposal.l2_blocks.len(),
                current_proposal.total_bytes
            );
            Ok(current_proposal)
        } else {
            Err(anyhow::anyhow!("No current proposal"))
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
                    "Removed L2 block from proposal: {} txs, {} bytes",
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
            self.finalize_current_proposal();
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
                created_at_sec: l2_block_timestamp_sec,
                pending_confirmation: false,
            });
        }

        if is_forced_inclusion {
            if coinbase == self.config.default_coinbase && self.can_add_forced_inclusion() {
                self.inc_forced_inclusion()?;
            } else {
                return Err(anyhow::anyhow!(
                    "recover_from: Cannot add FI block with coinbase {} to proposal",
                    coinbase
                ));
            }
        } else {
            if let Some(proposal) = self.current_proposal.as_mut()
                && proposal.anchor_block_id < anchor_info.id()
            {
                proposal.anchor_block_id = anchor_info.id();
                proposal.anchor_block_timestamp_sec = anchor_info.timestamp_sec();
                proposal.anchor_block_hash = anchor_info.hash();
                proposal.anchor_state_root = anchor_info.state_root();
            }

            let bytes_length =
                crate::shared::l2_tx_lists::encode_and_compress(&tx_list)?.len() as u64;

            if !self.can_fit_recovered_block(bytes_length) {
                return Err(anyhow::anyhow!(
                    "recover_from: block does not fit in proposal {} (adding {} bytes would exceed blob size limit). Reorg needed.",
                    proposal_id,
                    bytes_length,
                ));
            }

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

            // at previous step we check that proposal exists
            self.add_l2_block_and_get_current_proposal(l2_block)?;
        }
        Ok(())
    }

    pub fn inc_forced_inclusion(&mut self) -> Result<(), Error> {
        self.current_proposal
            .as_mut()
            .map(|proposal| proposal.num_forced_inclusion += 1)
            .ok_or_else(|| anyhow::anyhow!("No current proposal to add forced inclusion to"))
    }

    fn can_fit_recovered_block(&mut self, bytes_length: u64) -> bool {
        self.current_proposal.as_mut().is_some_and(|proposal| {
            let new_block_count = match u16::try_from(proposal.l2_blocks.len() + 1) {
                Ok(n) => n,
                Err(_) => return false,
            };

            let mut new_total_bytes = proposal.total_bytes + bytes_length;

            if !self.config.is_within_bytes_limit(new_total_bytes) {
                proposal.compress();
                new_total_bytes = proposal.total_bytes + bytes_length;
            }

            self.config.is_within_bytes_limit(new_total_bytes)
                && self.config.is_within_block_limit(new_block_count)
        })
    }

    fn is_same_proposal_id(&self, proposal_id: u64) -> bool {
        // Since Proposal has a public id field, we can access it directly
        self.current_proposal
            .as_ref()
            .is_some_and(|proposal| proposal.id == proposal_id)
    }

    pub fn is_empty(&self) -> bool {
        trace!(
            "proposal_builder::is_empty: current_proposal is none: {}, proposals_to_send len: {}",
            self.current_proposal.is_none(),
            self.proposals_to_send.len()
        );
        self.current_proposal.is_none() && self.proposals_to_send.is_empty()
    }

    /// Remove the front proposal if it was dispatched and the transaction monitor has finished.
    /// Must only be called when no transaction is in progress.
    pub fn remove_confirmed_proposal(&mut self) {
        if self
            .proposals_to_send
            .front()
            .is_some_and(|p| p.pending_confirmation)
        {
            self.proposals_to_send.pop_front();
        }
    }

    /// Mark the front proposal as not confirmed and ready to be resubmitted.
    /// Must only be called when no transaction is in progress.
    pub fn mark_not_confirmed_proposal_to_resubmit(&mut self) {
        if let Some(proposal) = self.proposals_to_send.front_mut() {
            if !proposal.pending_confirmation {
                tracing::error!(
                    "There is no pending confirmation proposal to mark as not confirmed."
                );
            }
            proposal.pending_confirmation = false;
        }
    }

    pub async fn try_submit_oldest_proposal(
        &mut self,
        ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
        submit_only_full_proposals: bool,
        l2_slot_timestamp: u64,
    ) -> Result<(), Error> {
        if let Some(current_proposal) = self.current_proposal.as_ref() {
            let block_count = u16::try_from(current_proposal.l2_blocks.len()).unwrap_or(0);
            let is_full = !self.config.is_within_block_limit(block_count + 1);
            let is_expired = !self
                .config
                .is_within_time_limit(current_proposal.created_at_sec, l2_slot_timestamp);

            if !submit_only_full_proposals || is_full || is_expired {
                self.finalize_current_proposal();
            }
        }

        let proposals_number = self.proposals_to_send.len();
        if let Some(proposal) = self.proposals_to_send.front_mut() {
            if proposal.pending_confirmation {
                return Err(anyhow::anyhow!(
                    "Cannot submit proposal with anchor_block_id {} (proposals_to_send = {}), proposal is pending confirmation.",
                    proposal.anchor_block_id,
                    proposals_number,
                ));
            }

            debug!(
                anchor_block_id = %proposal.anchor_block_id,
                coinbase = %proposal.coinbase,
                l2_blocks_len = %proposal.l2_blocks.len(),
                total_bytes = %proposal.total_bytes,
                proposals_to_send = %proposals_number,
                current_proposal = %self.current_proposal.is_some(),
                "Submitting proposal"
            );

            // Dispatches tx building + monitoring to a background task (returns immediately).
            // Build errors (EstimationFailed, etc.) are reported via error_notification_channel.
            ethereum_l1
                .execution_layer
                .send_proposal_to_l1(proposal.l2_blocks.clone(), proposal.num_forced_inclusion)
                .await?;

            // Mark the proposal as dispatched — it will be removed once the monitor confirms.
            proposal.pending_confirmation = true;
        }

        Ok(())
    }

    // TODO do we have that check in SC?
    pub fn is_time_shift_between_blocks_expiring(&self, current_l2_slot_timestamp: u64) -> bool {
        if let Some(current_proposal) = self.current_proposal.as_ref() {
            // current proposal is not empty
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

    pub fn clone_without_proposals(&self) -> Self {
        Self {
            config: self.config.clone(),
            proposals_to_send: VecDeque::new(),
            current_proposal: None,
            slot_clock: self.slot_clock.clone(),
            metrics: self.metrics.clone(),
        }
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

    pub fn take_proposals_to_send(&mut self) -> VecDeque<Proposal> {
        std::mem::take(&mut self.proposals_to_send)
    }

    pub fn prepend_proposals(&mut self, mut proposals: Proposals) {
        proposals.append(&mut self.proposals_to_send);
        self.proposals_to_send = proposals;
    }

    pub fn get_current_proposal_id(&self) -> Option<u64> {
        self.current_proposal.as_ref().map(|b| b.id)
    }

    pub fn try_finalize_current_proposal(&mut self) -> Result<(), Error> {
        // TODO handle forced inclusion
        self.finalize_current_proposal();
        Ok(())
    }

    /// Decreases the forced inclusion count and removes the current proposal if empty.
    pub fn decrease_forced_inclusion_count(&mut self) {
        if let Some(proposal) = self.current_proposal.as_mut() {
            proposal.num_forced_inclusion = proposal.num_forced_inclusion.saturating_sub(1);

            if proposal.l2_blocks.is_empty() && proposal.num_forced_inclusion == 0 {
                self.remove_current_proposal();
            }
        }
    }

    fn remove_current_proposal(&mut self) {
        self.current_proposal = None;
    }

    pub fn finalize_current_proposal(&mut self) {
        if let Some(proposal) = self.current_proposal.take()
            && !proposal.l2_blocks.is_empty()
        {
            self.proposals_to_send.push_back(proposal);
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
}
