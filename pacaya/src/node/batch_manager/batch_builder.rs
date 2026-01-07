use std::sync::Arc;

use super::config::BatchesToSend;
use crate::l1::bindings::BatchParams;
use crate::{
    l1::execution_layer::ExecutionLayer,
    metrics::Metrics,
    node::batch_manager::batch::Batch,
    shared::{l2_block::L2Block, l2_tx_lists::PreBuiltTxList},
};
use alloy::primitives::Address;
use anyhow::Error;
use common::{
    batch_builder::{BatchBuilderConfig, BatchBuilderCore, BatchLike},
    l1::{ethereum_l1::EthereumL1, slot_clock::SlotClock, transaction_error::TransactionError},
};
use tracing::{debug, error};

pub struct BatchBuilder {
    core: BatchBuilderCore<Batch, BatchParams>,
}

impl BatchBuilder {
    pub fn new(
        config: BatchBuilderConfig,
        slot_clock: Arc<SlotClock>,
        metrics: Arc<Metrics>,
    ) -> Self {
        Self {
            core: BatchBuilderCore::new(None, config, slot_clock, metrics),
        }
    }

    /// Returns a reference to the batch builder configuration.
    ///
    /// This configuration is used to manage batching parameters.
    pub fn get_config(&self) -> &BatchBuilderConfig {
        &self.core.config
    }

    pub fn can_consume_l2_block(&mut self, l2_block: &L2Block) -> bool {
        self.core.can_consume_l2_block(l2_block)
    }

    pub fn current_batch_is_empty(&self) -> bool {
        self.core
            .current_batch
            .as_ref()
            .is_none_or(|b| b.l2_blocks().is_empty())
    }

    pub fn try_finalize_current_batch(&mut self) -> Result<(), Error> {
        let is_empty = self
            .core
            .current_batch
            .as_ref()
            .is_none_or(|b| b.l2_blocks().is_empty());

        let has_forced_inclusion = self.core.current_forced_inclusion.is_some();

        if has_forced_inclusion && is_empty {
            error!(
                "Failed to finalize current batch, current_batch {} forced_inclusion {}",
                self.core.current_batch.is_some(),
                self.core.current_forced_inclusion.is_some()
            );
            return Err(anyhow::anyhow!(
                "Failed to finalize current batch, current_batch {} forced_inclusion {}",
                self.core.current_batch.is_some(),
                self.core.current_forced_inclusion.is_some()
            ));
        }
        self.core.finalize_current_batch();
        Ok(())
    }

    pub fn set_forced_inclusion(&mut self, forced_inclusion_batch: BatchParams) -> bool {
        if self.core.current_forced_inclusion.is_some() {
            return false;
        }
        self.core.current_forced_inclusion = Some(forced_inclusion_batch);
        true
    }

    pub fn create_new_batch(&mut self, anchor_block_id: u64, anchor_block_timestamp_sec: u64) {
        // TODO replace with try_finalize_current_batch
        self.core.finalize_current_batch();
        self.core.current_batch = Some(Batch {
            total_bytes: 0,
            l2_blocks: vec![],
            anchor_block_id,
            anchor_block_timestamp_sec,
            coinbase: self.core.config.default_coinbase,
        });
    }

    pub fn remove_current_batch(&mut self) {
        self.core.current_batch = None;
    }

    pub fn create_new_batch_and_add_l2_block(
        &mut self,
        anchor_block_id: u64,
        anchor_block_timestamp_sec: u64,
        l2_block: L2Block,
        coinbase: Option<Address>,
    ) {
        self.core.finalize_current_batch();
        self.core.current_batch = Some(Batch {
            total_bytes: l2_block.prebuilt_tx_list.bytes_length,
            l2_blocks: vec![l2_block],
            anchor_block_id,
            anchor_block_timestamp_sec,
            coinbase: coinbase.unwrap_or(self.core.config.default_coinbase),
        });
    }

    /// Returns true if the block was added to the batch, false otherwise.
    pub fn add_l2_block_and_get_current_anchor_block_id(
        &mut self,
        l2_block: L2Block,
    ) -> Result<u64, Error> {
        self.core.add_l2_block(l2_block)?;
        Ok(self
            .core
            .current_batch
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No current batch after adding L2 block"))?
            .anchor_block_id())
    }

    pub fn remove_last_l2_block(&mut self) {
        self.core.remove_last_l2_block();
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
            self.core.finalize_current_batch();
            self.core.current_batch = Some(Batch {
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
        self.core
            .current_batch
            .as_ref()
            .is_some_and(|batch| batch.anchor_block_id() == anchor_block_id)
    }

    fn is_same_coinbase(&self, coinbase: Address) -> bool {
        self.core
            .current_batch
            .as_ref()
            .is_some_and(|batch| batch.coinbase == coinbase)
    }

    pub async fn try_submit_oldest_batch(
        &mut self,
        ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
        submit_only_full_batches: bool,
    ) -> Result<(), Error> {
        if self.core.current_batch.is_some()
            && (!submit_only_full_batches
                || !self.core.config.is_within_block_limit(
                    u16::try_from(
                        self.core
                            .current_batch
                            .as_ref()
                            .map(|b| b.l2_blocks.len())
                            .unwrap_or(0),
                    )? + 1,
                ))
        {
            self.core.finalize_current_batch();
        }

        if let Some((forced_inclusion, batch)) = self.core.batches_to_send.front() {
            if ethereum_l1
                .execution_layer
                .is_transaction_in_progress()
                .await?
            {
                debug!(
                    batches_to_send = %self.core.batches_to_send.len(),
                    current_batch = %self.core.current_batch.is_some(),
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
                batches_to_send = %self.core.batches_to_send.len(),
                current_batch = %self.core.current_batch.is_some(),
                "Submitting batch"
            );

            if let Err(err) = ethereum_l1
                .execution_layer
                .send_batch_to_l1(
                    batch.l2_blocks.clone(),
                    batch.anchor_block_id,
                    batch.coinbase,
                    self.core.slot_clock.get_current_slot_begin_timestamp()?,
                    forced_inclusion.clone(),
                )
                .await
            {
                if let Some(transaction_error) = err.downcast_ref::<TransactionError>()
                    && !matches!(transaction_error, TransactionError::EstimationTooEarly)
                {
                    debug!("BatchBuilder: Transaction error, removing all batches");
                    self.core.batches_to_send.clear();
                }
                return Err(err);
            }

            self.core.batches_to_send.pop_front();
        }

        Ok(())
    }

    pub fn is_time_shift_expired(&self, current_l2_slot_timestamp: u64) -> bool {
        // These methods only need current_batch, no sync needed
        self.core.is_time_shift_expired(current_l2_slot_timestamp)
    }

    pub fn is_greater_than_max_anchor_height_offset(&self) -> Result<bool, Error> {
        // These methods only need current_batch, no sync needed
        self.core.is_greater_than_max_anchor_height_offset()
    }

    pub fn try_creating_l2_block(
        &mut self,
        pending_tx_list: Option<PreBuiltTxList>,
        l2_slot_timestamp: u64,
        end_of_sequencing: bool,
    ) -> Option<L2Block> {
        self.core
            .try_creating_l2_block(pending_tx_list, l2_slot_timestamp, end_of_sequencing)
    }

    pub fn clone_without_batches(&self) -> Self {
        Self {
            core: self.core.clone_without_batches(),
        }
    }

    pub fn get_number_of_batches(&self) -> u64 {
        self.core.get_number_of_batches()
    }

    pub fn get_number_of_batches_ready_to_send(&self) -> u64 {
        self.core.batches_to_send.len() as u64
    }

    pub fn take_batches_to_send(&mut self) -> BatchesToSend {
        std::mem::take(&mut self.core.batches_to_send)
    }

    pub fn prepend_batches(&mut self, mut batches: BatchesToSend) {
        batches.append(&mut self.core.batches_to_send);
        self.core.batches_to_send = batches;
    }

    pub fn is_empty(&self) -> bool {
        self.core.is_empty()
    }

    pub fn has_current_forced_inclusion(&self) -> bool {
        self.core.has_current_forced_inclusion()
    }

    pub fn finalize_current_batch(&mut self) {
        self.core.finalize_current_batch();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared;

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

        let slot_clock = Arc::new(SlotClock::new(0, 5, 12, 32, 3000));
        let mut batch_builder = BatchBuilder {
            core: BatchBuilderCore::new(
                Some(batch),
                config,
                slot_clock.clone(),
                Arc::new(Metrics::new()),
            ),
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

        let total_bytes = batch_builder
            .core
            .current_batch
            .as_ref()
            .unwrap()
            .total_bytes();
        (res, total_bytes)
    }

    #[test]
    fn test_can_consume_l2_block_with_single_compression() {
        let (res, total_bytes) = test_can_consume_l2_block(339);
        assert!(res);
        assert_eq!(total_bytes, 203);
    }

    #[test]
    fn test_can_consume_l2_block_with_double_compression() {
        let (res, total_bytes) = test_can_consume_l2_block(330);
        assert!(res);
        assert_eq!(total_bytes, 203);
    }

    #[test]
    fn test_can_not_consume_l2_block_with_compression() {
        let (res, total_bytes) = test_can_consume_l2_block(329);
        assert!(!res);
        assert_eq!(total_bytes, 203);
    }

    #[test]
    fn test_can_consume_l2_block_no_compression() {
        let (res, total_bytes) = test_can_consume_l2_block(1000);
        assert!(res);
        assert_eq!(total_bytes, 228 * 2);
    }
}
