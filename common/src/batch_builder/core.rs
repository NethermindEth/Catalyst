//! Common batch builder logic that works with any BatchLike type.

use crate::{
    batch_builder::{BatchBuilderConfig, BatchLike},
    l1::slot_clock::SlotClock,
    metrics::Metrics,
    shared::l2_block::L2Block,
    shared::l2_tx_lists::PreBuiltTxList,
};
use std::{collections::VecDeque, sync::Arc};
use tracing::{debug, trace, warn};

/// Core batch builder that encapsulates common logic for working with batches or proposals.
///
/// This struct owns the current batch/proposal and the queue of batches ready to send,
/// providing methods for common batch building operations.
pub struct BatchBuilderCore<B: BatchLike, F> {
    pub current_batch: Option<B>,
    pub current_forced_inclusion: Option<F>,
    pub batches_to_send: VecDeque<(Option<F>, B)>,
    pub config: BatchBuilderConfig,
    pub slot_clock: Arc<SlotClock>,
    pub metrics: Arc<Metrics>,
    last_l2_block_timestamp: u64,
}

impl<B: BatchLike, F> Drop for BatchBuilderCore<B, F> {
    fn drop(&mut self) {
        debug!(
            "BatchBuilder dropped! current_batch is none: {}, batches_to_send len: {}",
            self.current_batch.is_none(),
            self.batches_to_send.len()
        );
    }
}

impl<B: BatchLike, F> BatchBuilderCore<B, F> {
    /// Creates a new `BatchBuilderCore` instance.
    pub fn new(
        current_batch: Option<B>,
        config: BatchBuilderConfig,
        slot_clock: Arc<SlotClock>,
        metrics: Arc<Metrics>,
    ) -> Self {
        Self {
            current_batch,
            current_forced_inclusion: None,
            batches_to_send: VecDeque::new(),
            config,
            slot_clock,
            metrics,
            last_l2_block_timestamp: 0,
        }
    }

    /// Returns a reference to the batches queue.
    pub fn batches_to_send(&self) -> &VecDeque<(Option<F>, B)> {
        &self.batches_to_send
    }

    /// Returns a mutable reference to the batches queue.
    pub fn batches_to_send_mut(&mut self) -> &mut VecDeque<(Option<F>, B)> {
        &mut self.batches_to_send
    }

    /// Returns a reference to the current batch.
    pub fn current_batch(&self) -> Option<&B> {
        self.current_batch.as_ref()
    }

    /// Returns a mutable reference to the current batch.
    pub fn current_batch_mut(&mut self) -> Option<&mut B> {
        self.current_batch.as_mut()
    }

    pub fn has_current_forced_inclusion(&self) -> bool {
        self.current_forced_inclusion.is_some()
    }

    /// Checks if the time shift between blocks has expired.
    pub fn is_time_shift_expired(&self, current_l2_slot_timestamp: u64) -> bool {
        if let Some(batch) = self.current_batch.as_ref()
            && let Some(last_block) = batch.l2_blocks().last()
        {
            return current_l2_slot_timestamp - last_block.timestamp_sec
                > self.config.max_time_shift_between_blocks_sec;
        }
        false
    }

    /// Checks if the time shift between blocks is expiring soon.
    pub fn is_time_shift_between_blocks_expiring(&self, current_l2_slot_timestamp: u64) -> bool {
        if let Some(batch) = self.current_batch.as_ref()
            && let Some(last_block) = batch.l2_blocks().last()
        {
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
        false
    }

    /// Checks if this is the last L1 slot to add an empty L2 block.
    pub fn is_the_last_l1_slot_to_add_an_empty_l2_block(
        &self,
        current_l2_slot_timestamp: u64,
        last_block_timestamp: u64,
    ) -> bool {
        current_l2_slot_timestamp - last_block_timestamp
            >= self.config.max_time_shift_between_blocks_sec - self.config.l1_slot_duration_sec
    }

    /// Checks if the anchor height offset is greater than the maximum allowed.
    pub fn is_greater_than_max_anchor_height_offset(&self) -> Result<bool, anyhow::Error> {
        if let Some(batch) = self.current_batch.as_ref() {
            let slots_since_l1_block = self
                .slot_clock
                .slots_since_l1_block(batch.anchor_block_timestamp_sec())?;
            return Ok(slots_since_l1_block > self.config.max_anchor_height_offset);
        }
        Ok(false)
    }

    /// Determines if a new block should be created based on pending transactions and timing.
    pub fn should_new_block_be_created(
        &self,
        number_of_pending_txs: u64,
        current_l2_slot_timestamp: u64,
        end_of_sequencing: bool,
    ) -> bool {
        if self.is_empty_block_required(current_l2_slot_timestamp) || end_of_sequencing {
            return true;
        }

        if number_of_pending_txs >= self.config.preconf_min_txs {
            return true;
        }

        let number_of_l2_slots = (current_l2_slot_timestamp - self.last_l2_block_timestamp) * 1000
            / self.slot_clock.get_preconf_heartbeat_ms();
        number_of_l2_slots > self.config.preconf_max_skipped_l2_slots
    }

    /// Checks if an empty block is required to prevent time shift overflow.
    pub fn is_empty_block_required(&self, preconfirmation_timestamp: u64) -> bool {
        self.is_time_shift_between_blocks_expiring(preconfirmation_timestamp)
    }

    pub fn is_empty(&self) -> bool {
        trace!(
            "batch_builder::is_empty: current_batch is none: {}, batches_to_send len: {}",
            self.current_batch.is_none(),
            self.batches_to_send.len()
        );
        // Check both current_batch and the actual batches_to_send (which includes forced inclusion)
        self.current_batch.is_none() && self.batches_to_send.is_empty()
    }

    /// Returns the number of batches (current + queued).
    pub fn get_number_of_batches(&self) -> u64 {
        self.batches_to_send.len() as u64 + if self.current_batch.is_some() { 1 } else { 0 }
    }

    /// Returns the number of batches ready to send.
    pub fn get_number_of_batches_ready_to_send(&self) -> u64 {
        self.batches_to_send.len() as u64
    }

    /// Checks if a new L2 block can be consumed by the current batch.
    ///
    /// This function handles compression logic and checks size/block limits.
    pub fn can_consume_l2_block(&mut self, l2_block: &L2Block) -> bool {
        let is_time_shift_expired = self.is_time_shift_expired(l2_block.timestamp_sec);

        let Some(batch) = self.current_batch.as_mut() else {
            return false;
        };

        let new_block_count = match u16::try_from(batch.l2_blocks().len() + 1) {
            Ok(n) => n,
            Err(_) => return false,
        };

        let mut new_total_bytes = batch.total_bytes() + l2_block.prebuilt_tx_list.bytes_length;

        if !self.config.is_within_bytes_limit(new_total_bytes) {
            // first compression, compressing the batch without the new L2 block
            batch.compress();
            new_total_bytes = batch.total_bytes() + l2_block.prebuilt_tx_list.bytes_length;
            if !self.config.is_within_bytes_limit(new_total_bytes) {
                // second compression, compressing the batch with the new L2 block
                // we can tolerate the processing overhead as it's a very rare case
                let mut batch_clone = batch.clone();
                batch_clone.l2_blocks_mut().push(l2_block.clone());
                batch_clone.compress();
                new_total_bytes = batch_clone.total_bytes();
                tracing::debug!(
                    "can_consume_l2_block: Second compression, new total bytes: {}",
                    new_total_bytes
                );
            }
        }

        self.config.is_within_bytes_limit(new_total_bytes)
            && self.config.is_within_block_limit(new_block_count)
            && !is_time_shift_expired
    }

    pub fn try_creating_l2_block(
        &mut self,
        pending_tx_list: Option<PreBuiltTxList>,
        l2_slot_timestamp: u64,
        end_of_sequencing: bool,
    ) -> Option<L2Block> {
        let tx_list_len = pending_tx_list
            .as_ref()
            .map(|tx_list| tx_list.tx_list.len())
            .unwrap_or(0);

        if !self.should_new_block_be_created(
            tx_list_len as u64,
            l2_slot_timestamp,
            end_of_sequencing,
        ) {
            debug!("Skipping preconfirmation for the current L2 slot");
            self.metrics.inc_skipped_l2_slots_by_low_txs_count();
            return None;
        }

        if let Some(pending_tx_list) = pending_tx_list {
            tracing::debug!(
                "Creating new block with pending tx list length: {}, bytes length: {}",
                pending_tx_list.tx_list.len(),
                pending_tx_list.bytes_length
            );
            Some(L2Block::new_from(pending_tx_list, l2_slot_timestamp))
        } else {
            Some(L2Block::new_empty(l2_slot_timestamp))
        }
    }

    pub fn remove_last_l2_block(&mut self) {
        if let Some(current_proposal) = self.current_batch.as_mut() {
            let removed_block = current_proposal.l2_blocks_mut().pop();
            if let Some(removed_block) = removed_block {
                *current_proposal.total_bytes_mut() -= removed_block.prebuilt_tx_list.bytes_length;
                if current_proposal.l2_blocks().is_empty() {
                    self.current_batch = None;
                }
                debug!(
                    "Removed L2 block from batch: {} txs, {} bytes",
                    removed_block.prebuilt_tx_list.tx_list.len(),
                    removed_block.prebuilt_tx_list.bytes_length
                );
            }
        }
    }

    pub fn clone_without_batches(&self) -> Self {
        Self {
            current_batch: None,
            current_forced_inclusion: None,
            batches_to_send: VecDeque::new(),
            config: self.config.clone(),
            slot_clock: self.slot_clock.clone(),
            metrics: self.metrics.clone(),
            last_l2_block_timestamp: 0,
        }
    }

    pub fn finalize_current_batch(&mut self) {
        if let Some(batch) = self.current_batch.take()
            && !batch.l2_blocks().is_empty()
        {
            self.batches_to_send
                .push_back((self.current_forced_inclusion.take(), batch.clone()));
        }
    }

    pub fn add_l2_block(&mut self, l2_block: L2Block) -> Result<(), anyhow::Error> {
        if let Some(current_batch) = self.current_batch.as_mut() {
            *current_batch.total_bytes_mut() += l2_block.prebuilt_tx_list.bytes_length;
            self.last_l2_block_timestamp = l2_block.timestamp_sec;
            current_batch.l2_blocks_mut().push(l2_block);
            debug!(
                "Added L2 block to batch: l2 blocks: {}, total bytes: {}",
                current_batch.l2_blocks().len(),
                current_batch.total_bytes()
            );
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "No current batch while adding L2 block to batch builder core"
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::Address;

    // Simple test batch type that implements BatchLike
    #[derive(Clone)]
    struct TestBatch {
        l2_blocks: Vec<L2Block>,
        total_bytes: u64,
        anchor_block_id: u64,
        anchor_block_timestamp_sec: u64,
    }

    impl BatchLike for TestBatch {
        fn l2_blocks_mut(&mut self) -> &mut Vec<L2Block> {
            &mut self.l2_blocks
        }

        fn l2_blocks(&self) -> &Vec<L2Block> {
            &self.l2_blocks
        }

        fn total_bytes_mut(&mut self) -> &mut u64 {
            &mut self.total_bytes
        }

        fn total_bytes(&self) -> u64 {
            self.total_bytes
        }

        fn anchor_block_id(&self) -> u64 {
            self.anchor_block_id
        }

        fn anchor_block_timestamp_sec(&self) -> u64 {
            self.anchor_block_timestamp_sec
        }

        fn compress(&mut self) {
            // No-op for tests
        }
    }

    #[test]
    fn test_is_the_last_l1_slot_to_add_an_empty_l2_block() {
        let batch_builder = BatchBuilderCore::<TestBatch, ()>::new(
            None,
            BatchBuilderConfig {
                max_bytes_size_of_batch: 1000,
                max_blocks_per_batch: 10,
                l1_slot_duration_sec: 12,
                max_time_shift_between_blocks_sec: 255,
                max_anchor_height_offset: 10,
                default_coinbase: Address::ZERO,
                preconf_min_txs: 5,
                preconf_max_skipped_l2_slots: 3,
            },
            Arc::new(SlotClock::new(0, 5, 12, 32, 3000)),
            Arc::new(Metrics::new()),
        );

        assert!(!batch_builder.is_the_last_l1_slot_to_add_an_empty_l2_block(100, 0));
        assert!(!batch_builder.is_the_last_l1_slot_to_add_an_empty_l2_block(242, 0));
        assert!(batch_builder.is_the_last_l1_slot_to_add_an_empty_l2_block(243, 0));
        assert!(batch_builder.is_the_last_l1_slot_to_add_an_empty_l2_block(255, 0));
    }

    #[test]
    fn test_should_new_block_be_created() {
        let config = BatchBuilderConfig {
            max_bytes_size_of_batch: 1000,
            max_blocks_per_batch: 10,
            l1_slot_duration_sec: 12,
            max_time_shift_between_blocks_sec: 255,
            max_anchor_height_offset: 10,
            default_coinbase: Address::ZERO,
            preconf_min_txs: 5,
            preconf_max_skipped_l2_slots: 3,
        };

        let slot_clock = Arc::new(SlotClock::new(0, 5, 12, 32, 2000));
        let mut core = BatchBuilderCore::<TestBatch, ()>::new(
            None,
            config,
            slot_clock,
            Arc::new(Metrics::new()),
        );
        core.last_l2_block_timestamp = 998;

        // Test case 1: Should create new block when pending transactions >= preconf_min_txs
        assert!(core.should_new_block_be_created(5, 1000, false));
        assert!(core.should_new_block_be_created(10, 1000, false));

        // Test case 2: Should not create new block when pending transactions < preconf_min_txs and no current batch
        assert!(!core.should_new_block_be_created(3, 1000, false));

        // Test case 3: Should not create new block when pending transactions < preconf_min_txs and current batch exists but no blocks
        let empty_batch = TestBatch {
            l2_blocks: vec![],
            total_bytes: 0,
            anchor_block_id: 0,
            anchor_block_timestamp_sec: 0,
        };
        core.current_batch = Some(empty_batch);
        assert!(!core.should_new_block_be_created(3, 1000, false));

        let l2_block = L2Block {
            prebuilt_tx_list: PreBuiltTxList {
                tx_list: vec![],
                estimated_gas_used: 0,
                bytes_length: 0,
            },
            timestamp_sec: 1000,
        };
        core.add_l2_block(l2_block).unwrap();

        // Test case 4: Should create new block when skipped slots > preconf_max_skipped_l2_slots
        assert!(core.should_new_block_be_created(0, 1008, false));

        // Test case 5: Should not create new block when skipped slots <= preconf_max_skipped_l2_slots
        assert!(!core.should_new_block_be_created(3, 1006, false));

        // Test case 6: Should create new block when end_of_sequencing is true
        assert!(core.should_new_block_be_created(3, 1006, true));

        // Test case 7: Should not create new block when is_empty_block_required is false
        assert!(!core.should_new_block_be_created(0, 1006, false));

        // Test case 8: Should create new block when is_empty_block_required is true
        assert!(core.should_new_block_be_created(0, 1260, false));

        // Test case 9: Should create new block when is_empty_block_required is true and end_of_sequencing is true
        assert!(core.should_new_block_be_created(0, 1260, true));
    }
}
