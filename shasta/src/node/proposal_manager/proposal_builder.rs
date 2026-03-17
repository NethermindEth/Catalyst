use std::{collections::VecDeque, sync::Arc};

use super::proposal::Proposals;
use super::proposal_queue::ProposalQueue;
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
    queue: ProposalQueue,
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
            queue: ProposalQueue::new(),
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
            .and_then(|p| p.last_block_timestamp())
    }

    pub fn remove_last_l2_block(&mut self) {
        if let Some(proposal) = self.current_proposal.as_mut() {
            proposal.remove_last_l2_block();
            if proposal.l2_blocks.is_empty() {
                self.current_proposal = None;
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

            // at previous step we check that proposal exists
            self.add_l2_block_and_get_current_proposal(l2_block)?;
        }
        Ok(())
    }

    pub fn inc_forced_inclusion(&mut self) -> Result<(), Error> {
        self.current_proposal
            .as_mut()
            .map(|proposal| proposal.inc_forced_inclusion())
            .ok_or_else(|| anyhow::anyhow!("No current proposal to add forced inclusion to"))
    }

    fn is_same_proposal_id(&self, proposal_id: u64) -> bool {
        // Since Proposal has a public id field, we can access it directly
        self.current_proposal
            .as_ref()
            .is_some_and(|proposal| proposal.id == proposal_id)
    }

    pub fn is_empty(&self) -> bool {
        trace!(
            "proposal_builder::is_empty: current_proposal is none: {}, queue len: {}",
            self.current_proposal.is_none(),
            self.queue.len()
        );
        self.current_proposal.is_none() && self.queue.is_empty()
    }

    /// Remove the front proposal if it was dispatched and the transaction monitor has finished.
    /// Must only be called when no transaction is in progress.
    pub fn remove_confirmed_proposal(&mut self) {
        self.queue.remove_confirmed();
    }

    /// Mark the front proposal as not confirmed and ready to be resubmitted.
    /// Must only be called when no transaction is in progress.
    pub fn mark_not_confirmed_proposal_to_resubmit(&mut self) {
        self.queue.mark_front_for_resubmit();
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

        let proposals_number = self.queue.len();
        if let Some(proposal) = self.queue.front_mut() {
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
        if let Some(current_proposal) = self.current_proposal.as_ref()
            && let Some(last_block) = current_proposal.l2_blocks.last()
        {
            if current_l2_slot_timestamp < last_block.timestamp_sec {
                warn!("Preconfirmation timestamp is before the last block timestamp");
                return false;
            }
            return common::batch_builder::is_last_slot_for_empty_block(
                current_l2_slot_timestamp,
                last_block.timestamp_sec,
                self.config.max_time_shift_between_blocks_sec,
                self.config.l1_slot_duration_sec,
            );
        }
        false
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
            queue: ProposalQueue::new(),
            current_proposal: None,
            slot_clock: self.slot_clock.clone(),
            metrics: self.metrics.clone(),
        }
    }

    pub fn get_number_of_proposals(&self) -> u64 {
        self.queue.len()
            + if self.current_proposal.is_some() {
                1
            } else {
                0
            }
    }

    pub fn get_number_of_proposals_ready_to_send(&self) -> u64 {
        self.queue.len()
    }

    pub fn take_proposals_to_send(&mut self) -> VecDeque<Proposal> {
        self.queue.take_all()
    }

    pub fn prepend_proposals(&mut self, proposals: Proposals) {
        self.queue.prepend(proposals);
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
            proposal.decrease_forced_inclusion_count();
            if proposal.is_empty() {
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
            self.queue.push(proposal);
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
            .is_some_and(|p| p.has_forced_inclusion())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{B256, Uint};
    use common::l1::slot_clock::SlotClock;
    use common::metrics::Metrics;

    const COINBASE: Address = Address::ZERO;

    fn make_tx() -> alloy::rpc::types::Transaction {
        serde_json::from_str(
            r#"{
            "blockHash":"0x0000000000000000000000000000000000000000000000000000000000000000",
            "blockNumber":"0x1",
            "from":"0x0000000000000000000000000000000000000001",
            "gas":"0x5208",
            "gasPrice":"0x1",
            "hash":"0x0000000000000000000000000000000000000000000000000000000000000001",
            "input":"0x",
            "nonce":"0x0",
            "to":"0x0000000000000000000000000000000000000002",
            "transactionIndex":"0x0",
            "value":"0x0",
            "type":"0x2",
            "accessList":[],
            "chainId":"0x1",
            "maxFeePerGas":"0x1",
            "maxPriorityFeePerGas":"0x0",
            "v":"0x0",
            "r":"0x0000000000000000000000000000000000000000000000000000000000000000",
            "s":"0x0000000000000000000000000000000000000000000000000000000000000000",
            "yParity":"0x0"
        }"#,
        )
        .expect("valid test tx json")
    }

    fn make_config() -> BatchBuilderConfig {
        BatchBuilderConfig {
            max_bytes_size_of_batch: 10_000,
            max_blocks_per_batch: 10,
            l1_slot_duration_sec: 12,
            max_time_shift_between_blocks_sec: 255,
            max_anchor_height_offset: 64,
            default_coinbase: COINBASE,
            preconf_min_txs: 3,
            preconf_max_skipped_l2_slots: 5,
            proposal_max_time_sec: 120,
        }
    }

    fn make_builder() -> ProposalBuilder {
        make_builder_with_config(make_config())
    }

    fn make_builder_with_config(config: BatchBuilderConfig) -> ProposalBuilder {
        let slot_clock = Arc::new(SlotClock::new(0, 0, 12, 32, 3000));
        let metrics = Arc::new(Metrics::new());
        ProposalBuilder::new(config, slot_clock, metrics)
    }

    fn make_anchor(id: u64, timestamp_sec: u64) -> AnchorBlockInfo {
        AnchorBlockInfo::new(id, timestamp_sec, B256::ZERO, B256::ZERO)
    }

    fn make_draft_block(timestamp: u64, bytes_len: u64) -> L2BlockV2Draft {
        L2BlockV2Draft {
            prebuilt_tx_list: PreBuiltTxList {
                tx_list: vec![],
                estimated_gas_used: 0,
                bytes_length: bytes_len,
            },
            timestamp_sec: timestamp,
            gas_limit_without_anchor: 1_000_000,
        }
    }

    fn make_checkpoint() -> Checkpoint {
        Checkpoint {
            blockNumber: Uint::from(100),
            blockHash: B256::ZERO,
            stateRoot: B256::ZERO,
        }
    }

    fn create_proposal(builder: &mut ProposalBuilder, id: u64, anchor_id: u64, timestamp: u64) {
        builder.create_new_proposal(id, make_anchor(anchor_id, timestamp), timestamp);
    }

    // --- Proposal lifecycle ---

    #[test]
    fn test_create_new_proposal() {
        let mut builder = make_builder();
        assert!(builder.is_empty());

        create_proposal(&mut builder, 1, 100, 1000);

        assert!(!builder.is_empty());
        assert_eq!(builder.get_current_proposal_id(), Some(1));
        assert_eq!(builder.get_number_of_proposals(), 1);
        assert_eq!(builder.get_number_of_proposals_ready_to_send(), 0);
    }

    #[test]
    fn test_create_new_proposal_finalizes_previous() {
        let mut builder = make_builder();

        create_proposal(&mut builder, 1, 100, 1000);
        let _ = builder.add_l2_draft_block(make_draft_block(1001, 100));

        create_proposal(&mut builder, 2, 101, 1012);

        assert_eq!(builder.get_current_proposal_id(), Some(2));
        assert_eq!(builder.get_number_of_proposals(), 2);
        assert_eq!(builder.get_number_of_proposals_ready_to_send(), 1);
    }

    #[test]
    fn test_finalize_empty_proposal_is_noop() {
        let mut builder = make_builder();
        create_proposal(&mut builder, 1, 100, 1000);

        builder.finalize_current_proposal();

        assert!(builder.is_empty());
        assert_eq!(builder.get_number_of_proposals_ready_to_send(), 0);
    }

    #[test]
    fn test_finalize_proposal_with_blocks_moves_to_queue() {
        let mut builder = make_builder();
        create_proposal(&mut builder, 1, 100, 1000);
        let _ = builder.add_l2_draft_block(make_draft_block(1001, 100));

        builder.finalize_current_proposal();

        assert_eq!(builder.get_current_proposal_id(), None);
        assert_eq!(builder.get_number_of_proposals_ready_to_send(), 1);
    }

    // --- Block addition ---

    #[test]
    fn test_add_l2_draft_block() {
        let mut builder = make_builder();
        create_proposal(&mut builder, 1, 100, 1000);

        let payload = builder
            .add_l2_draft_block(make_draft_block(1001, 200))
            .expect("should add block");

        assert_eq!(payload.proposal_id, 1);
        assert_eq!(payload.coinbase, COINBASE);
        assert_eq!(payload.timestamp_sec, 1001);
        assert_eq!(payload.anchor_block_id, 100);
        assert!(!payload.is_forced_inclusion);
        assert_eq!(
            builder.get_current_proposal_last_block_timestamp(),
            Some(1001)
        );
    }

    #[test]
    fn test_add_l2_draft_block_without_proposal_errors() {
        let mut builder = make_builder();
        let result = builder.add_l2_draft_block(make_draft_block(1001, 200));
        assert!(result.is_err());
    }

    #[test]
    fn test_add_fi_block() {
        let mut builder = make_builder();
        create_proposal(&mut builder, 1, 100, 1000);

        let payload = builder
            .add_fi_block(make_draft_block(1001, 50), make_checkpoint())
            .expect("should add FI block");

        assert_eq!(payload.proposal_id, 1);
        assert!(payload.is_forced_inclusion);
        assert_eq!(payload.anchor_block_id, 100);
        assert!(builder.has_current_forced_inclusion());
    }

    #[test]
    fn test_add_fi_block_without_proposal_errors() {
        let mut builder = make_builder();
        let result = builder.add_fi_block(make_draft_block(1001, 50), make_checkpoint());
        assert!(result.is_err());
    }

    // --- Capacity checks ---

    #[test]
    fn test_can_consume_l2_block_within_limits() {
        let mut builder = make_builder();
        create_proposal(&mut builder, 1, 100, 1000);
        let _ = builder.add_l2_draft_block(make_draft_block(1001, 100));

        assert!(builder.can_consume_l2_block(&make_draft_block(1002, 100)));
    }

    #[test]
    fn test_can_consume_l2_block_exceeds_byte_limit() {
        let mut config = make_config();
        // Even after compression, the manifest overhead exceeds 1 byte
        config.max_bytes_size_of_batch = 1;
        let mut builder = make_builder_with_config(config);
        create_proposal(&mut builder, 1, 100, 1000);
        let _ = builder.add_l2_draft_block(make_draft_block(1001, 50));

        assert!(!builder.can_consume_l2_block(&make_draft_block(1002, 50)));
    }

    #[test]
    fn test_can_consume_l2_block_exceeds_block_limit() {
        let mut config = make_config();
        config.max_blocks_per_batch = 2;
        let mut builder = make_builder_with_config(config);
        create_proposal(&mut builder, 1, 100, 1000);
        let _ = builder.add_l2_draft_block(make_draft_block(1001, 100));
        let _ = builder.add_l2_draft_block(make_draft_block(1002, 100));

        assert!(!builder.can_consume_l2_block(&make_draft_block(1003, 100)));
    }

    #[test]
    fn test_can_consume_l2_block_no_proposal() {
        let mut builder = make_builder();
        assert!(!builder.can_consume_l2_block(&make_draft_block(1001, 100)));
    }

    #[test]
    fn test_can_add_forced_inclusion_empty_proposal() {
        let mut builder = make_builder();
        create_proposal(&mut builder, 1, 100, 1000);

        assert!(builder.can_add_forced_inclusion());
    }

    #[test]
    fn test_can_add_forced_inclusion_with_blocks() {
        let mut builder = make_builder();
        create_proposal(&mut builder, 1, 100, 1000);
        let _ = builder.add_l2_draft_block(make_draft_block(1001, 100));

        assert!(!builder.can_add_forced_inclusion());
    }

    #[test]
    fn test_can_add_forced_inclusion_no_proposal() {
        let builder = make_builder();
        assert!(!builder.can_add_forced_inclusion());
    }

    // --- Block creation decision ---

    #[test]
    fn test_should_new_block_be_created_enough_txs() {
        let builder = make_builder();
        let tx_list = Some(PreBuiltTxList {
            tx_list: vec![make_tx(), make_tx(), make_tx()],
            estimated_gas_used: 0,
            bytes_length: 0,
        });

        assert!(builder.should_new_block_be_created(&tx_list, 1000, false));
    }

    #[test]
    fn test_should_new_block_be_created_not_enough_txs() {
        let mut builder = make_builder();
        create_proposal(&mut builder, 1, 100, 1000);
        let _ = builder.add_l2_draft_block(make_draft_block(1000, 100));

        let tx_list = Some(PreBuiltTxList {
            tx_list: vec![make_tx()],
            estimated_gas_used: 0,
            bytes_length: 0,
        });

        assert!(!builder.should_new_block_be_created(&tx_list, 1001, false));
    }

    #[test]
    fn test_should_new_block_be_created_end_of_sequencing() {
        let builder = make_builder();
        assert!(builder.should_new_block_be_created(&None, 1000, true));
    }

    #[test]
    fn test_should_new_block_be_created_time_shift_expiring() {
        let mut builder = make_builder();
        create_proposal(&mut builder, 1, 100, 1000);
        let _ = builder.add_l2_draft_block(make_draft_block(1000, 100));

        // max_time_shift_between_blocks_sec=255, l1_slot_duration_sec=12
        // threshold = 255 - 12 = 243
        let timestamp = 1000 + 243;
        assert!(builder.should_new_block_be_created(&None, timestamp, false));
    }

    #[test]
    fn test_should_new_block_be_created_skipped_slots() {
        let mut builder = make_builder();
        create_proposal(&mut builder, 1, 100, 1000);
        let _ = builder.add_l2_draft_block(make_draft_block(1000, 100));

        // preconf_heartbeat_ms=3000, preconf_max_skipped_l2_slots=5
        // number_of_l2_slots = (ts_diff * 1000) / 3000
        // need number_of_l2_slots > 5  =>  ts_diff * 1000 / 3000 > 5  =>  ts_diff >= 18
        assert!(!builder.should_new_block_be_created(&None, 1015, false));
        assert!(builder.should_new_block_be_created(&None, 1018, false));
    }

    #[test]
    fn test_should_new_block_be_created_no_proposal_no_txs() {
        let builder = make_builder();
        assert!(builder.should_new_block_be_created(&None, 1000, false));
    }

    // --- Time shift ---

    #[test]
    fn test_time_shift_between_blocks_expiring() {
        let mut builder = make_builder();
        create_proposal(&mut builder, 1, 100, 1000);
        let _ = builder.add_l2_draft_block(make_draft_block(1000, 100));

        assert!(!builder.is_time_shift_between_blocks_expiring(1100));
        assert!(builder.is_time_shift_between_blocks_expiring(1243));
        assert!(builder.is_time_shift_between_blocks_expiring(1255));
    }

    #[test]
    fn test_time_shift_empty_proposal() {
        let mut builder = make_builder();
        create_proposal(&mut builder, 1, 100, 1000);

        assert!(!builder.is_time_shift_between_blocks_expiring(2000));
    }

    #[test]
    fn test_time_shift_no_proposal() {
        let builder = make_builder();
        assert!(!builder.is_time_shift_between_blocks_expiring(2000));
    }

    #[test]
    fn test_time_shift_timestamp_before_last_block() {
        let mut builder = make_builder();
        create_proposal(&mut builder, 1, 100, 1000);
        let _ = builder.add_l2_draft_block(make_draft_block(1000, 100));

        assert!(!builder.is_time_shift_between_blocks_expiring(999));
    }

    // --- Cleanup & state ---

    #[test]
    fn test_remove_last_l2_block() {
        let mut builder = make_builder();
        create_proposal(&mut builder, 1, 100, 1000);
        let _ = builder.add_l2_draft_block(make_draft_block(1001, 200));
        let _ = builder.add_l2_draft_block(make_draft_block(1002, 300));

        builder.remove_last_l2_block();

        assert_eq!(builder.get_current_proposal_id(), Some(1));
        assert_eq!(
            builder.get_current_proposal_last_block_timestamp(),
            Some(1001)
        );
    }

    #[test]
    fn test_remove_last_l2_block_removes_empty_proposal() {
        let mut builder = make_builder();
        create_proposal(&mut builder, 1, 100, 1000);
        let _ = builder.add_l2_draft_block(make_draft_block(1001, 200));

        builder.remove_last_l2_block();

        assert_eq!(builder.get_current_proposal_id(), None);
        assert!(builder.is_empty());
    }

    #[test]
    fn test_decrease_forced_inclusion_count() {
        let mut builder = make_builder();
        create_proposal(&mut builder, 1, 100, 1000);
        let _ = builder.add_fi_block(make_draft_block(1001, 50), make_checkpoint());

        assert!(builder.has_current_forced_inclusion());

        builder.decrease_forced_inclusion_count();

        assert!(!builder.has_current_forced_inclusion());
        assert_eq!(builder.get_current_proposal_id(), None);
    }

    #[test]
    fn test_decrease_forced_inclusion_keeps_proposal_with_blocks() {
        let mut builder = make_builder();
        create_proposal(&mut builder, 1, 100, 1000);
        let _ = builder.inc_forced_inclusion();
        let _ = builder.add_l2_draft_block(make_draft_block(1001, 100));

        builder.decrease_forced_inclusion_count();

        assert_eq!(builder.get_current_proposal_id(), Some(1));
    }

    // --- Submission queue ---

    #[test]
    fn test_finalize_and_take_proposals() {
        let mut builder = make_builder();
        create_proposal(&mut builder, 1, 100, 1000);
        let _ = builder.add_l2_draft_block(make_draft_block(1001, 100));
        builder.finalize_current_proposal();

        create_proposal(&mut builder, 2, 101, 1012);
        let _ = builder.add_l2_draft_block(make_draft_block(1013, 100));
        builder.finalize_current_proposal();

        let proposals = builder.take_proposals_to_send();
        assert_eq!(proposals.len(), 2);
        assert_eq!(proposals[0].id, 1);
        assert_eq!(proposals[1].id, 2);
        assert_eq!(builder.get_number_of_proposals_ready_to_send(), 0);
    }

    #[test]
    fn test_remove_confirmed_proposal() {
        let mut builder = make_builder();
        create_proposal(&mut builder, 1, 100, 1000);
        let _ = builder.add_l2_draft_block(make_draft_block(1001, 100));
        builder.finalize_current_proposal();

        builder
            .queue
            .front_mut()
            .expect("has proposal")
            .pending_confirmation = true;
        builder.remove_confirmed_proposal();

        assert_eq!(builder.get_number_of_proposals_ready_to_send(), 0);
    }

    #[test]
    fn test_remove_confirmed_proposal_skips_unconfirmed() {
        let mut builder = make_builder();
        create_proposal(&mut builder, 1, 100, 1000);
        let _ = builder.add_l2_draft_block(make_draft_block(1001, 100));
        builder.finalize_current_proposal();

        builder.remove_confirmed_proposal();

        assert_eq!(builder.get_number_of_proposals_ready_to_send(), 1);
    }

    #[test]
    fn test_mark_not_confirmed_to_resubmit() {
        let mut builder = make_builder();
        create_proposal(&mut builder, 1, 100, 1000);
        let _ = builder.add_l2_draft_block(make_draft_block(1001, 100));
        builder.finalize_current_proposal();

        builder
            .queue
            .front_mut()
            .expect("has proposal")
            .pending_confirmation = true;
        builder.mark_not_confirmed_proposal_to_resubmit();

        let front = builder.queue.front_mut().expect("has proposal");
        assert!(!front.pending_confirmation);
    }

    #[test]
    fn test_prepend_proposals() {
        let mut builder = make_builder();
        create_proposal(&mut builder, 2, 101, 1012);
        let _ = builder.add_l2_draft_block(make_draft_block(1013, 100));
        builder.finalize_current_proposal();

        let mut earlier = VecDeque::new();
        earlier.push_back(Proposal {
            id: 1,
            l2_blocks: vec![L2BlockV2::new_empty(1001, COINBASE, 100, 1_000_000)],
            ..Proposal::default()
        });

        builder.prepend_proposals(earlier);

        assert_eq!(builder.get_number_of_proposals_ready_to_send(), 2);
        let proposals = builder.take_proposals_to_send();
        assert_eq!(proposals[0].id, 1);
        assert_eq!(proposals[1].id, 2);
    }

    // --- Recovery ---

    #[tokio::test]
    async fn test_recover_from_creates_new_proposal() {
        let mut builder = make_builder();
        let anchor = make_anchor(100, 1000);

        builder
            .recover_from(1, anchor, COINBASE, vec![], 1001, 1_000_000, false)
            .await
            .expect("should recover");

        assert_eq!(builder.get_current_proposal_id(), Some(1));
        assert_eq!(
            builder.get_current_proposal_last_block_timestamp(),
            Some(1001)
        );
    }

    #[tokio::test]
    async fn test_recover_from_same_proposal_adds_block() {
        let mut builder = make_builder();
        let anchor = make_anchor(100, 1000);

        builder
            .recover_from(1, anchor, COINBASE, vec![], 1001, 1_000_000, false)
            .await
            .expect("first recover");

        let anchor2 = make_anchor(100, 1000);
        builder
            .recover_from(1, anchor2, COINBASE, vec![], 1002, 1_000_000, false)
            .await
            .expect("second recover");

        assert_eq!(builder.get_current_proposal_id(), Some(1));
        assert_eq!(
            builder.get_current_proposal_last_block_timestamp(),
            Some(1002)
        );
    }

    #[tokio::test]
    async fn test_recover_from_different_proposal_finalizes_previous() {
        let mut builder = make_builder();
        let anchor = make_anchor(100, 1000);

        builder
            .recover_from(1, anchor, COINBASE, vec![], 1001, 1_000_000, false)
            .await
            .expect("first recover");

        let anchor2 = make_anchor(101, 1012);
        builder
            .recover_from(2, anchor2, COINBASE, vec![], 1013, 1_000_000, false)
            .await
            .expect("second recover");

        assert_eq!(builder.get_current_proposal_id(), Some(2));
        assert_eq!(builder.get_number_of_proposals_ready_to_send(), 1);
    }

    #[tokio::test]
    async fn test_recover_from_forced_inclusion() {
        let mut builder = make_builder();
        let anchor = make_anchor(100, 1000);

        builder
            .recover_from(1, anchor, COINBASE, vec![], 1001, 1_000_000, true)
            .await
            .expect("recover FI");

        assert!(builder.has_current_forced_inclusion());
        assert_eq!(builder.get_current_proposal_id(), Some(1));
    }

    #[tokio::test]
    async fn test_recover_from_forced_inclusion_wrong_coinbase_errors() {
        let mut builder = make_builder();
        let anchor = make_anchor(100, 1000);
        let wrong_coinbase = Address::new([1u8; 20]);

        let result = builder
            .recover_from(1, anchor, wrong_coinbase, vec![], 1001, 1_000_000, true)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_recover_updates_anchor_when_newer() {
        let mut builder = make_builder();
        let anchor = make_anchor(100, 1000);

        builder
            .recover_from(1, anchor, COINBASE, vec![], 1001, 1_000_000, false)
            .await
            .expect("first recover");

        let newer_anchor = make_anchor(105, 1060);
        builder
            .recover_from(1, newer_anchor, COINBASE, vec![], 1002, 1_000_000, false)
            .await
            .expect("second recover with newer anchor");

        let proposal = builder.current_proposal.as_ref().expect("has proposal");
        assert_eq!(proposal.anchor_block_id, 105);
    }

    // --- Clone without proposals ---

    #[test]
    fn test_clone_without_proposals() {
        let mut builder = make_builder();
        create_proposal(&mut builder, 1, 100, 1000);
        let _ = builder.add_l2_draft_block(make_draft_block(1001, 100));
        builder.finalize_current_proposal();
        create_proposal(&mut builder, 2, 101, 1012);

        let cloned = builder.clone_without_proposals();

        assert!(cloned.is_empty());
        assert_eq!(cloned.get_current_proposal_id(), None);
        assert_eq!(cloned.get_number_of_proposals_ready_to_send(), 0);
    }

    // --- Inc forced inclusion ---

    #[test]
    fn test_inc_forced_inclusion_no_proposal_errors() {
        let mut builder = make_builder();
        assert!(builder.inc_forced_inclusion().is_err());
    }

    #[test]
    fn test_inc_forced_inclusion() {
        let mut builder = make_builder();
        create_proposal(&mut builder, 1, 100, 1000);

        builder.inc_forced_inclusion().expect("should inc");

        assert!(builder.has_current_forced_inclusion());
    }
}
