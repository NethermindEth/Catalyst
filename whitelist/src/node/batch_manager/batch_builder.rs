use std::{collections::VecDeque, sync::Arc};

use super::config::{BatchesToSend, ForcedInclusionBatch};
use crate::{
    ethereum_l1::{
        EthereumL1, extension::ELExtension, slot_clock::SlotClock,
        transaction_error::TransactionError,
    },
    metrics::Metrics,
    node::batch_manager::{batch::Batch, config::BatchBuilderConfig},
    shared::{l2_block::L2Block, l2_tx_lists::PreBuiltTxList},
};
use alloy::primitives::Address;
use anyhow::Error;
use tracing::{debug, error, trace, warn};

pub struct BatchBuilder {
    config: BatchBuilderConfig,
    batches_to_send: BatchesToSend,
    current_batch: Option<Batch>,
    current_forced_inclusion: ForcedInclusionBatch,
    slot_clock: Arc<SlotClock>,
    metrics: Arc<Metrics>,
}

impl Drop for BatchBuilder {
    fn drop(&mut self) {
        debug!(
            "BatchBuilder dropped! current_batch is none: {}, batches_to_send len: {}",
            self.current_batch.is_none(),
            self.batches_to_send.len()
        );
    }
}

impl BatchBuilder {
    pub fn new(
        config: BatchBuilderConfig,
        slot_clock: Arc<SlotClock>,
        metrics: Arc<Metrics>,
    ) -> Self {
        Self {
            config,
            batches_to_send: VecDeque::new(),
            current_batch: None,
            current_forced_inclusion: None,
            slot_clock,
            metrics,
        }
    }

    /// Returns a reference to the batch builder configuration.
    ///
    /// This configuration is used to manage batching parameters.
    pub fn get_config(&self) -> &BatchBuilderConfig {
        &self.config
    }

    pub fn can_consume_l2_block(&mut self, l2_block: &L2Block) -> bool {
        let is_time_shift_expired = self.is_time_shift_expired(l2_block.timestamp_sec);
        self.current_batch.as_mut().is_some_and(|batch| {
            let new_block_count = match u16::try_from(batch.l2_blocks.len() + 1) {
                Ok(n) => n,
                Err(_) => return false,
            };

            let mut new_total_bytes = batch.total_bytes + l2_block.prebuilt_tx_list.bytes_length;

            if !self.config.is_within_bytes_limit(new_total_bytes) {
                // first compression, compressing the batch without the new L2 block
                batch.compress();
                new_total_bytes = batch.total_bytes + l2_block.prebuilt_tx_list.bytes_length;
                if !self.config.is_within_bytes_limit(new_total_bytes) {
                    // second compression, compressing the batch with the new L2 block
                    // we can tolerate the processing overhead as it's a very rare case
                    let mut batch_clone = batch.clone();
                    batch_clone.l2_blocks.push(l2_block.clone());
                    batch_clone.compress();
                    new_total_bytes = batch_clone.total_bytes;
                    debug!(
                        "can_consume_l2_block: Second compression, new total bytes: {}",
                        new_total_bytes
                    );
                }
            }

            self.config.is_within_bytes_limit(new_total_bytes)
                && self.config.is_within_block_limit(new_block_count)
                && !is_time_shift_expired
        })
    }

    pub fn finalize_current_batch(&mut self) {
        if let Some(batch) = self.current_batch.take() {
            if !batch.l2_blocks.is_empty() {
                self.batches_to_send
                    .push_back((self.current_forced_inclusion.take(), batch));
            }
        }
    }

    pub fn has_current_forced_inclusion(&self) -> bool {
        self.current_forced_inclusion.is_some()
    }

    pub fn try_finalize_current_batch(&mut self) -> Result<(), Error> {
        let is_empty = self
            .current_batch
            .as_ref()
            .is_none_or(|b| b.l2_blocks.is_empty());

        let has_forced_inclusion = self.current_forced_inclusion.is_some();

        if has_forced_inclusion && is_empty {
            error!(
                "Failed to finalize current batch, current_batch {} forced_inclusion {}",
                self.current_batch.is_some(),
                self.current_forced_inclusion.is_some()
            );
            return Err(anyhow::anyhow!(
                "Failed to finalize current batch, current_batch {} forced_inclusion {}",
                self.current_batch.is_some(),
                self.current_forced_inclusion.is_some()
            ));
        }
        self.finalize_current_batch();
        Ok(())
    }

    pub fn set_forced_inclusion(&mut self, forced_inclusion_batch: ForcedInclusionBatch) -> bool {
        if self.current_forced_inclusion.is_some() {
            return false;
        }
        self.current_forced_inclusion = forced_inclusion_batch;
        true
    }

    pub fn create_new_batch(&mut self, anchor_block_id: u64, anchor_block_timestamp_sec: u64) {
        self.finalize_current_batch();
        self.current_batch = Some(Batch {
            total_bytes: 0,
            l2_blocks: vec![],
            anchor_block_id,
            anchor_block_timestamp_sec,
            coinbase: self.config.default_coinbase,
        });
    }

    pub fn remove_current_batch(&mut self) {
        self.current_batch = None;
    }

    pub fn create_new_batch_and_add_l2_block(
        &mut self,
        anchor_block_id: u64,
        anchor_block_timestamp_sec: u64,
        l2_block: L2Block,
        coinbase: Option<Address>,
    ) {
        self.finalize_current_batch();
        self.current_batch = Some(Batch {
            total_bytes: l2_block.prebuilt_tx_list.bytes_length,
            l2_blocks: vec![l2_block],
            anchor_block_id,
            anchor_block_timestamp_sec,
            coinbase: coinbase.unwrap_or(self.config.default_coinbase),
        });
    }

    /// Returns true if the block was added to the batch, false otherwise.
    pub fn add_l2_block_and_get_current_anchor_block_id(
        &mut self,
        l2_block: L2Block,
    ) -> Result<u64, Error> {
        if let Some(current_batch) = self.current_batch.as_mut() {
            current_batch.total_bytes += l2_block.prebuilt_tx_list.bytes_length;
            current_batch.l2_blocks.push(l2_block);
            debug!(
                "Added L2 block to batch: l2 blocks: {}, total bytes: {}",
                current_batch.l2_blocks.len(),
                current_batch.total_bytes
            );
            Ok(current_batch.anchor_block_id)
        } else {
            Err(anyhow::anyhow!("No current batch"))
        }
    }

    pub fn remove_last_l2_block(&mut self) {
        if let Some(current_batch) = self.current_batch.as_mut() {
            let removed_block = current_batch.l2_blocks.pop();
            if let Some(removed_block) = removed_block {
                current_batch.total_bytes -= removed_block.prebuilt_tx_list.bytes_length;
                if current_batch.l2_blocks.is_empty() {
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

    pub fn recover_from(
        &mut self,
        tx_list: Vec<alloy::rpc::types::Transaction>,
        anchor_block_id: u64,
        anchor_block_timestamp_sec: u64,
        l2_block_timestamp_sec: u64,
        coinbase: Address,
    ) -> Result<(), Error> {
        // We have a new batch if any of the following is true:
        // 1. Anchor block IDs differ
        // 2. Time difference between two blocks exceeds u8
        if !self.is_same_anchor_block_id(anchor_block_id)
            || self.is_time_shift_expired(l2_block_timestamp_sec)
            || !self.is_same_coinbase(coinbase)
        {
            self.finalize_current_batch();
            self.current_batch = Some(Batch {
                total_bytes: 0,
                l2_blocks: vec![],
                anchor_block_id,
                coinbase,
                anchor_block_timestamp_sec,
            });
        }

        let bytes_length = crate::shared::l2_tx_lists::encode_and_compress(&tx_list)?.len() as u64;
        let l2_block = L2Block::new_from(
            crate::shared::l2_tx_lists::PreBuiltTxList {
                tx_list,
                estimated_gas_used: 0,
                bytes_length,
            },
            l2_block_timestamp_sec,
        );

        if self.can_consume_l2_block(&l2_block) {
            self.add_l2_block_and_get_current_anchor_block_id(l2_block)?;
        } else {
            self.create_new_batch_and_add_l2_block(
                anchor_block_id,
                anchor_block_timestamp_sec,
                l2_block,
                Some(coinbase),
            );
        }

        Ok(())
    }

    fn is_same_anchor_block_id(&self, anchor_block_id: u64) -> bool {
        self.current_batch
            .as_ref()
            .is_some_and(|batch| batch.anchor_block_id == anchor_block_id)
    }

    fn is_same_coinbase(&self, coinbase: Address) -> bool {
        self.current_batch
            .as_ref()
            .is_some_and(|batch| batch.coinbase == coinbase)
    }

    pub fn is_empty(&self) -> bool {
        trace!(
            "batch_builder::is_empty: current_batch is none: {}, batches_to_send len: {}",
            self.current_batch.is_none(),
            self.batches_to_send.len()
        );
        self.current_batch.is_none() && self.batches_to_send.is_empty()
    }

    pub async fn try_submit_oldest_batch<T: ELExtension>(
        &mut self,
        ethereum_l1: Arc<EthereumL1<T>>,
        submit_only_full_batches: bool,
    ) -> Result<(), Error> {
        if self.current_batch.is_some()
            && (!submit_only_full_batches
                || !self.config.is_within_block_limit(
                    u16::try_from(
                        self.current_batch
                            .as_ref()
                            .map(|b| b.l2_blocks.len())
                            .unwrap_or(0),
                    )? + 1,
                ))
        {
            self.finalize_current_batch();
        }

        if let Some((forced_inclusion, batch)) = self.batches_to_send.front() {
            if ethereum_l1
                .execution_layer
                .is_transaction_in_progress()
                .await?
            {
                debug!(
                    batches_to_send = %self.batches_to_send.len(),
                    current_batch = %self.current_batch.is_some(),
                    "Cannot submit batch, transaction is in progress.",
                );
                return Ok(());
            }

            debug!(
                anchor_block_id = %batch.anchor_block_id,
                coinbase = %batch.coinbase,
                l2_blocks_len = %batch.l2_blocks.len(),
                total_bytes = %batch.total_bytes,
                batches_to_send = %self.batches_to_send.len(),
                current_batch = %self.current_batch.is_some(),
                "Submitting batch"
            );

            if let Err(err) = ethereum_l1
                .execution_layer
                .send_batch_to_l1(
                    batch.l2_blocks.clone(),
                    batch.anchor_block_id,
                    batch.coinbase,
                    self.slot_clock.get_current_slot_begin_timestamp()?,
                    forced_inclusion.clone(),
                )
                .await
            {
                if let Some(transaction_error) = err.downcast_ref::<TransactionError>() {
                    if !matches!(transaction_error, TransactionError::EstimationTooEarly) {
                        debug!("BatchBuilder: Transaction error, removing all batches");
                        self.batches_to_send.clear();
                    }
                }
                return Err(err);
            }

            self.batches_to_send.pop_front();
        }

        Ok(())
    }

    pub fn is_time_shift_expired(&self, current_l2_slot_timestamp: u64) -> bool {
        if let Some(current_batch) = self.current_batch.as_ref() {
            if let Some(last_block) = current_batch.l2_blocks.last() {
                return current_l2_slot_timestamp - last_block.timestamp_sec
                    > self.config.max_time_shift_between_blocks_sec;
            }
        }
        false
    }

    pub fn is_time_shift_between_blocks_expiring(&self, current_l2_slot_timestamp: u64) -> bool {
        if let Some(current_batch) = self.current_batch.as_ref() {
            // l1_batches is not empty
            if let Some(last_block) = current_batch.l2_blocks.last() {
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

    fn is_the_last_l1_slot_to_add_an_empty_l2_block(
        &self,
        current_l2_slot_timestamp: u64,
        last_block_timestamp: u64,
    ) -> bool {
        current_l2_slot_timestamp - last_block_timestamp
            >= self.config.max_time_shift_between_blocks_sec - self.config.l1_slot_duration_sec
    }

    pub fn is_greater_than_max_anchor_height_offset(&self) -> Result<bool, Error> {
        if let Some(current_batch) = self.current_batch.as_ref() {
            let slots_since_l1_block = self
                .slot_clock
                .slots_since_l1_block(current_batch.anchor_block_timestamp_sec)?;
            return Ok(slots_since_l1_block > self.config.max_anchor_height_offset);
        }
        Ok(false)
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
        if self.should_new_block_be_created(
            tx_list_len as u64,
            l2_slot_timestamp,
            end_of_sequencing,
        ) {
            if let Some(pending_tx_list) = pending_tx_list {
                debug!(
                    "Creating new block with pending tx list length: {}, bytes length: {}",
                    pending_tx_list.tx_list.len(),
                    pending_tx_list.bytes_length
                );

                Some(L2Block::new_from(pending_tx_list, l2_slot_timestamp))
            } else {
                Some(L2Block::new_empty(l2_slot_timestamp))
            }
        } else {
            debug!("Skipping preconfirmation for the current L2 slot");
            self.metrics.inc_skipped_l2_slots_by_low_txs_count();
            None
        }
    }

    fn should_new_block_be_created(
        &self,
        number_of_pending_txs: u64,
        current_l2_slot_timestamp: u64,
        end_of_sequencing: bool,
    ) -> bool {
        if self.is_empty_block_required(current_l2_slot_timestamp) || end_of_sequencing {
            return true;
        }

        if number_of_pending_txs == 0 {
            return false;
        }

        if number_of_pending_txs >= self.config.preconf_min_txs {
            return true;
        }

        if let Some(current_batch) = self.current_batch.as_ref()
            && let Some(last_block) = current_batch.l2_blocks.last()
        {
            let number_of_l2_slots = (current_l2_slot_timestamp - last_block.timestamp_sec) * 1000
                / self.slot_clock.get_preconf_heartbeat_ms();
            return number_of_l2_slots > self.config.preconf_max_skipped_l2_slots;
        }

        true
    }

    fn is_empty_block_required(&self, preconfirmation_timestamp: u64) -> bool {
        self.is_time_shift_between_blocks_expiring(preconfirmation_timestamp)
    }

    pub fn clone_without_batches(&self) -> Self {
        Self {
            config: self.config.clone(),
            batches_to_send: VecDeque::new(),
            current_batch: None,
            current_forced_inclusion: None,
            slot_clock: self.slot_clock.clone(),
            metrics: self.metrics.clone(),
        }
    }

    pub fn get_number_of_batches(&self) -> u64 {
        self.batches_to_send.len() as u64 + if self.current_batch.is_some() { 1 } else { 0 }
    }

    pub fn get_number_of_batches_ready_to_send(&self) -> u64 {
        self.batches_to_send.len() as u64
    }

    pub fn take_batches_to_send(&mut self) -> BatchesToSend {
        std::mem::take(&mut self.batches_to_send)
    }

    pub fn prepend_batches(&mut self, mut batches: BatchesToSend) {
        batches.append(&mut self.batches_to_send);
        self.batches_to_send = batches;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared;

    #[test]
    fn test_is_the_last_l1_slot_to_add_an_empty_l2_block() {
        let batch_builder = BatchBuilder::new(
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

    fn build_tx_1() -> alloy::rpc::types::Transaction {
        let json_data = r#"
        {
            "blockHash":"0x347bf1fbeab30fb516012c512222e229dfded991a2f1ba469f31c4273eb18921",
            "blockNumber":"0x5",
            "from":"0x0000777735367b36bc9b61c50022d9d0700db4ec",
            "gas":"0xf4240",
            "gasPrice":"0x86ff51",
            "maxFeePerGas":"0x86ff51",
            "maxPriorityFeePerGas":"0x0",
            "hash":"0xc921473ec8d6e93a9e499f4a5c7619fa9cc6ea8f24c89ad338f6c4095347af5c",
            "input":"0x48080a450000000000000000000000000000000000000000000000000000000000000146ef85e2f713b8212f4ff858962a5a5a0a1193b4033d702301cf5b68e29c7bffe6000000000000000000000000000000000000000000000000000000000001d28e0000000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000000004b00000000000000000000000000000000000000000000000000000000004c4b40000000000000000000000000000000000000000000000000000000004fdec7000000000000000000000000000000000000000000000000000000000023c3460000000000000000000000000000000000000000000000000000000000000001200000000000000000000000000000000000000000000000000000000000000000",
            "nonce":"0x4",
            "to":"0x1670010000000000000000000000000000010001",
            "transactionIndex":"0x0",
            "value":"0x0",
            "type":"0x2",
            "accessList":[],
            "chainId":"0x28c59",
            "v":"0x0",
            "r":"0x79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
            "s":"0xa8c3e2979dec89d4c055ffc1c900d33731cb43f027e427dff52a6ddf1247ec5",
            "yParity":"0x0"
        }"#;

        let tx: alloy::rpc::types::Transaction = serde_json::from_str(json_data).unwrap();
        tx
    }

    fn build_tx_2() -> alloy::rpc::types::Transaction {
        let json_data = r#"
        {
            "blockHash":"0x347bf1fbeab30fb516012c512222e229dfded991a2f1ba469f31c4273eb18921",
            "blockNumber":"0x5",
            "from":"0x8943545177806ed17b9f23f0a21ee5948ecaa776",
            "gas":"0x33450",
            "gasPrice":"0x77bc9351",
            "maxFeePerGas":"0x6fc23ac00",
            "maxPriorityFeePerGas":"0x77359400",
            "hash":"0x71e6a604469d2dd04175e195500b0811b3ecb6b005f19e724cbfd27050ac8e69",
            "input":"0x",
            "nonce":"0x4",
            "to":"0x5291a539174785fadc93effe9c9ceb7a54719ae4",
            "transactionIndex":"0x1",
            "value":"0x1550f7dca70000",
            "type":"0x2",
            "accessList":[],
            "chainId":"0x28c59",
            "v":"0x1",
            "r":"0x6c31bcf74110a61e6c82aa18aaca29bdd7c33807c2eee18d81c7f73617cc1728",
            "s":"0x31d38525206dc1926590d0ccae89ec3427ff9ef7851e58ef619111c9fbece8c",
            "yParity":"0x1"
        }"#;

        let tx: alloy::rpc::types::Transaction = serde_json::from_str(json_data).unwrap();
        tx
    }

    fn test_can_consume_l2_block(max_bytes_size_of_batch: u64) -> (bool, u64) {
        let config = BatchBuilderConfig {
            max_bytes_size_of_batch,
            max_blocks_per_batch: 10,
            l1_slot_duration_sec: 12,
            max_time_shift_between_blocks_sec: 255,
            max_anchor_height_offset: 10,
            default_coinbase: Address::ZERO,
            preconf_min_txs: 5,
            preconf_max_skipped_l2_slots: 3,
        };

        let mut batch = Batch {
            l2_blocks: vec![], //Vec<L2Block>,
            total_bytes: 228 * 2,
            coinbase: Address::ZERO,
            anchor_block_id: 0,
            anchor_block_timestamp_sec: 0,
        };

        let tx1 = build_tx_1();

        let l2_block = L2Block {
            prebuilt_tx_list: shared::l2_tx_lists::PreBuiltTxList {
                tx_list: vec![tx1.clone(), tx1],
                estimated_gas_used: 0,
                bytes_length: 228 * 2,
            },
            timestamp_sec: 0,
        };
        batch.l2_blocks.push(l2_block);

        let mut batch_builder = BatchBuilder {
            config,
            current_batch: Some(batch),
            batches_to_send: VecDeque::new(),
            current_forced_inclusion: None,
            slot_clock: Arc::new(SlotClock::new(0, 5, 12, 32, 3000)),
            metrics: Arc::new(Metrics::new()),
        };

        let tx2 = build_tx_2();

        let l2_block = L2Block {
            prebuilt_tx_list: shared::l2_tx_lists::PreBuiltTxList {
                tx_list: vec![tx2],
                estimated_gas_used: 0,
                bytes_length: 136,
            },
            timestamp_sec: 0,
        };

        let res = batch_builder.can_consume_l2_block(&l2_block);

        let total_bytes = batch_builder.current_batch.as_ref().unwrap().total_bytes;
        (res, total_bytes)
    }

    #[test]
    fn test_can_not_consume_l2_block_with_compression() {
        let (res, total_bytes) = test_can_consume_l2_block(366);
        assert!(!res);
        assert_eq!(total_bytes, 242);
    }

    #[test]
    fn test_can_consume_l2_block_with_single_compression() {
        let (res, total_bytes) = test_can_consume_l2_block(378);
        assert!(res);
        assert_eq!(total_bytes, 242);
    }

    #[test]
    fn test_can_consume_l2_block_with_double_compression() {
        let (res, total_bytes) = test_can_consume_l2_block(367);
        assert!(res);
        assert_eq!(total_bytes, 242);
    }

    #[test]
    fn test_can_consume_l2_block_no_compression() {
        let (res, total_bytes) = test_can_consume_l2_block(1000);
        assert!(res);
        assert_eq!(total_bytes, 228 * 2);
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
        let mut batch_builder = BatchBuilder::new(config, slot_clock, Arc::new(Metrics::new()));

        // Test case 1: Should create new block when pending transactions >= preconf_min_txs
        assert!(batch_builder.should_new_block_be_created(5, 1000, false));
        assert!(batch_builder.should_new_block_be_created(10, 1000, false));

        // Test case 2: Should create new block when pending transactions < preconf_min_txs and no current batch
        assert!(batch_builder.should_new_block_be_created(3, 1000, false));

        // Test case 3: Should create new block when pending transactions < preconf_min_txs and current batch exists but no blocks
        let empty_batch = Batch {
            l2_blocks: vec![],
            total_bytes: 0,
            coinbase: Address::ZERO,
            anchor_block_id: 0,
            anchor_block_timestamp_sec: 0,
        };
        batch_builder.current_batch = Some(empty_batch);
        assert!(batch_builder.should_new_block_be_created(3, 1000, false));

        let batch_with_blocks = Batch {
            l2_blocks: vec![L2Block {
                prebuilt_tx_list: shared::l2_tx_lists::PreBuiltTxList {
                    tx_list: vec![],
                    estimated_gas_used: 0,
                    bytes_length: 0,
                },
                timestamp_sec: 1000,
            }],
            total_bytes: 0,
            coinbase: Address::ZERO,
            anchor_block_id: 0,
            anchor_block_timestamp_sec: 0,
        };
        batch_builder.current_batch = Some(batch_with_blocks);

        // Test case 4: Should create new block when skipped slots > preconf_max_skipped_l2_slots
        assert!(batch_builder.should_new_block_be_created(3, 1008, false));

        // Test case 5: Should not create new block when skipped slots <= preconf_max_skipped_l2_slots
        assert!(!batch_builder.should_new_block_be_created(3, 1006, false));

        // Test case 6: Should create new block when end_of_sequencing is true
        assert!(batch_builder.should_new_block_be_created(3, 1006, true));

        // Test case 7: Should not create new block when is_empty_block_required is false
        assert!(!batch_builder.should_new_block_be_created(0, 1008, false));

        // Test case 8: Should create new block when is_empty_block_required is true
        assert!(batch_builder.should_new_block_be_created(0, 1260, false));

        // Test case 9: Should create new block when is_empty_block_required is true and end_of_sequencing is true
        assert!(batch_builder.should_new_block_be_created(0, 1260, true));
    }
}
