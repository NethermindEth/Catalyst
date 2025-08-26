pub mod batch;
mod batch_builder;
pub mod config;

use crate::{
    forced_inclusion::ForcedInclusion,
    l1::execution_layer::ExecutionLayer,
    metrics::Metrics,
    node::batch_manager::config::BatchesToSend,
    shared::{l2_block::L2Block, l2_slot_info::L2SlotInfo, l2_tx_lists::PreBuiltTxList},
};
use alloy::rpc::types::Transaction as GethTransaction;
use alloy::{consensus::BlockHeader, consensus::Transaction, primitives::Address};
use anyhow::Error;
use batch_builder::BatchBuilder;
use common::{
    l1::ethereum_l1::EthereumL1,
    l2::{
        self, operation_type::OperationType, preconf_blocks::BuildPreconfBlockResponse, taiko,
        taiko::Taiko,
    },
};
use config::BatchBuilderConfig;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

// Temporary struct while we don't have forced inclusion flag in extra data
#[derive(PartialEq)]
enum CachedForcedInclusion {
    Empty,
    NoData,
    Txs(Vec<GethTransaction>),
}

pub struct BatchManager {
    batch_builder: BatchBuilder,
    ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
    pub taiko: Arc<Taiko<ExecutionLayer>>,
    l1_height_lag: u64,
    forced_inclusion: Arc<ForcedInclusion>,
    cached_forced_inclusion_txs: CachedForcedInclusion,
    metrics: Arc<Metrics>,
}

impl BatchManager {
    pub fn new(
        l1_height_lag: u64,
        config: BatchBuilderConfig,
        ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
        taiko: Arc<Taiko<ExecutionLayer>>,
        metrics: Arc<Metrics>,
    ) -> Self {
        info!(
            "Batch builder config:\n\
             max_bytes_size_of_batch: {}\n\
             max_blocks_per_batch: {}\n\
             l1_slot_duration_sec: {}\n\
             max_time_shift_between_blocks_sec: {}\n\
             max_anchor_height_offset: {}",
            config.max_bytes_size_of_batch,
            config.max_blocks_per_batch,
            config.l1_slot_duration_sec,
            config.max_time_shift_between_blocks_sec,
            config.max_anchor_height_offset,
        );
        let forced_inclusion = Arc::new(ForcedInclusion::new(ethereum_l1.clone()));
        Self {
            batch_builder: BatchBuilder::new(
                config,
                ethereum_l1.slot_clock.clone(),
                metrics.clone(),
            ),
            ethereum_l1,
            taiko,
            l1_height_lag,
            forced_inclusion,
            cached_forced_inclusion_txs: CachedForcedInclusion::Empty,
            metrics,
        }
    }

    fn compare_transactions_list(tx1: &[GethTransaction], tx2: &[GethTransaction]) -> bool {
        tx1.len() == tx2.len()
            && tx1
                .iter()
                .zip(tx2.iter())
                .all(|(a, b)| a.inner.hash() == b.inner.hash())
    }

    pub async fn is_forced_inclusion(
        &mut self,
        block_id: u64,
        txs: &[GethTransaction],
    ) -> Result<bool, Error> {
        let is_forced_inclusion = match self
            .taiko
            .get_forced_inclusion_form_l1origin(block_id)
            .await
        {
            Ok(fi) => fi,
            Err(e) => {
                warn!("Failed to get forced inclusion from Taiko Geth: {}", e);
                // TODO remove it once geth updated on all networks
                match &self.cached_forced_inclusion_txs {
                    CachedForcedInclusion::NoData => false,
                    CachedForcedInclusion::Empty => {
                        if let Some(fi) = self
                            .forced_inclusion
                            .decode_current_forced_inclusion()
                            .await?
                        {
                            let res = BatchManager::compare_transactions_list(&fi.txs, txs);
                            self.cached_forced_inclusion_txs = CachedForcedInclusion::Txs(fi.txs);
                            res
                        } else {
                            self.cached_forced_inclusion_txs = CachedForcedInclusion::NoData;
                            false
                        }
                    }
                    CachedForcedInclusion::Txs(cached_txs) => {
                        BatchManager::compare_transactions_list(cached_txs, txs)
                    }
                }
            }
        };

        Ok(is_forced_inclusion)
    }

    pub async fn check_and_handle_forced_inclusion(
        &mut self,
        block_id: u64,
        txs: &[GethTransaction],
        coinbase: Address,
        anchor_block_id: u64,
        timestamp: u64,
    ) -> Result<bool, Error> {
        let forced_inclusion = self.is_forced_inclusion(block_id, txs).await?;
        debug!(
            "Handle forced inclusion: is forced inclusion: {}",
            forced_inclusion
        );

        if forced_inclusion {
            self.batch_builder.try_finalize_current_batch()?;
            let forced_inclusion = self.forced_inclusion.consume_forced_inclusion().await?;
            self.cached_forced_inclusion_txs = CachedForcedInclusion::Empty;
            if let Some(forced_inclusion) = forced_inclusion {
                let forced_inclusion_batch = self
                    .ethereum_l1
                    .execution_layer
                    .extension
                    .build_forced_inclusion_batch(
                        coinbase,
                        anchor_block_id,
                        timestamp,
                        &forced_inclusion,
                    );
                // set it to batch builder
                if !self
                    .batch_builder
                    .set_forced_inclusion(Some(forced_inclusion_batch))
                {
                    error!("Failed to set forced inclusion batch");
                    return Err(anyhow::anyhow!("Failed to set forced inclusion batch"));
                }
                debug!("Created forced inclusion batch while recovering from L2 block");
                return Ok(true);
            } else {
                return Err(anyhow::anyhow!("Failed to get next forced inclusion data"));
            }
        }

        Ok(false)
    }

    pub async fn recover_from_l2_block(&mut self, block_height: u64) -> Result<(), Error> {
        debug!("Recovering from L2 block {}", block_height);
        let block = self
            .taiko
            .get_l2_block_by_number(block_height, true)
            .await?;
        let (anchor_tx, txs) = match block.transactions.as_transactions() {
            Some(txs) => txs
                .split_first()
                .ok_or_else(|| anyhow::anyhow!("Cannot get anchor transaction from block"))?,
            None => return Err(anyhow::anyhow!("No transactions in block")),
        };

        let coinbase = block.header.beneficiary();

        let anchor_block_id = taiko::decode_anchor_id_from_tx_data(anchor_tx.input())?;
        debug!(
            "Recovering from L2 block {}, anchor block id {}, timestamp {}, coinbase {}, transactions {}",
            block_height,
            anchor_block_id,
            block.header.timestamp,
            coinbase,
            txs.len()
        );

        let anchor_block_timestamp_sec = self
            .ethereum_l1
            .execution_layer
            .get_block_timestamp_by_number(anchor_block_id)
            .await?;

        let txs = txs.to_vec();
        let forced_inclusion_handled = self
            .check_and_handle_forced_inclusion(
                block_height,
                &txs,
                coinbase,
                anchor_block_id,
                block.header.timestamp,
            )
            .await?;

        if !forced_inclusion_handled {
            self.batch_builder.recover_from(
                txs,
                anchor_block_id,
                anchor_block_timestamp_sec,
                block.header.timestamp,
                coinbase,
            )?;
        } else {
            debug!("Forced inclusion handled block id: {}", block.header.number);
        }
        Ok(())
    }

    pub async fn get_l1_anchor_block_offset_for_l2_block(
        &self,
        l2_block_height: u64,
    ) -> Result<u64, Error> {
        debug!(
            "get_anchor_block_offset: Checking L2 block {}",
            l2_block_height
        );
        let block = self
            .taiko
            .get_l2_block_by_number(l2_block_height, false)
            .await?;

        let anchor_tx_hash = block
            .transactions
            .as_hashes()
            .and_then(|txs| txs.first())
            .ok_or_else(|| anyhow::anyhow!("get_anchor_block_offset: No transactions in block"))?;

        let l2_anchor_tx = self.taiko.get_transaction_by_hash(*anchor_tx_hash).await?;
        let l1_anchor_block_id = l2::taiko::decode_anchor_id_from_tx_data(l2_anchor_tx.input())?;

        debug!(
            "get_l1_anchor_block_offset_for_l2_block: L2 block {l2_block_height} has L1 anchor block id {l1_anchor_block_id}"
        );

        self.ethereum_l1.slot_clock.slots_since_l1_block(
            self.ethereum_l1
                .execution_layer
                .get_block_timestamp_by_number(l1_anchor_block_id)
                .await?,
        )
    }

    pub fn is_anchor_block_offset_valid(&self, anchor_block_offset: u64) -> bool {
        anchor_block_offset
            < self
                .ethereum_l1
                .execution_layer
                .get_config_max_anchor_height_offset()
    }

    pub async fn reanchor_block(
        &mut self,
        pending_tx_list: PreBuiltTxList,
        l2_slot_info: L2SlotInfo,
        is_forced_inclusion: bool,
        allow_forced_inclusion: bool,
    ) -> Result<Option<BuildPreconfBlockResponse>, Error> {
        let l2_block = L2Block::new_from(pending_tx_list, l2_slot_info.slot_timestamp());

        if is_forced_inclusion && allow_forced_inclusion {
            // skip forced inclusion block because we had OldestForcedInclusionDue
            return Ok(None);
        }

        let block = if is_forced_inclusion {
            self.preconfirm_forced_inclusion_block(l2_slot_info, OperationType::Reanchor)
                .await?
        } else {
            let (_, block) = self
                .add_new_l2_block(
                    l2_block,
                    l2_slot_info,
                    false,
                    OperationType::Reanchor,
                    allow_forced_inclusion,
                )
                .await?;
            block
        };

        Ok(block)
    }

    pub async fn preconfirm_block(
        &mut self,
        pending_tx_list: Option<PreBuiltTxList>,
        l2_slot_info: L2SlotInfo,
        end_of_sequencing: bool,
        allow_forced_inclusion: bool,
    ) -> Result<
        (
            Option<BuildPreconfBlockResponse>,
            Option<BuildPreconfBlockResponse>,
        ),
        Error,
    > {
        let result = if let Some(l2_block) = self.batch_builder.try_creating_l2_block(
            pending_tx_list,
            l2_slot_info.slot_timestamp(),
            end_of_sequencing,
        ) {
            self.add_new_l2_block(
                l2_block,
                l2_slot_info,
                end_of_sequencing,
                OperationType::Preconfirm,
                allow_forced_inclusion,
            )
            .await?
        } else {
            (None, None)
        };

        if self
            .batch_builder
            .is_greater_than_max_anchor_height_offset()?
        {
            // Handle max anchor height offset exceeded
            info!("📈 Maximum allowed anchor height offset exceeded, finalizing current batch.");
            self.batch_builder.finalize_current_batch();
        }

        Ok(result)
    }

    async fn preconfirm_forced_inclusion_block(
        &mut self,
        l2_slot_info: L2SlotInfo,
        operation_type: OperationType,
    ) -> Result<Option<BuildPreconfBlockResponse>, Error> {
        let anchor_block_id = self.calculate_anchor_block_id().await?;

        let start = std::time::Instant::now();
        let forced_inclusion = self.forced_inclusion.consume_forced_inclusion().await?;
        self.cached_forced_inclusion_txs = CachedForcedInclusion::Empty;
        debug!(
            "Got forced inclusion in {} milliseconds",
            start.elapsed().as_millis()
        );

        if let Some(forced_inclusion) = forced_inclusion {
            let forced_inclusion_batch = self
                .ethereum_l1
                .execution_layer
                .extension
                .build_forced_inclusion_batch(
                    self.batch_builder.get_config().default_coinbase,
                    anchor_block_id,
                    l2_slot_info.slot_timestamp(),
                    &forced_inclusion,
                );
            // preconfirm
            let forced_inclusion_block = L2Block {
                prebuilt_tx_list: PreBuiltTxList {
                    tx_list: forced_inclusion.txs,
                    estimated_gas_used: 0,
                    bytes_length: 0,
                },
                timestamp_sec: l2_slot_info.slot_timestamp(),
            };
            let preconfed_block = match self
                .taiko
                .advance_head_to_new_l2_block(
                    forced_inclusion_block,
                    anchor_block_id,
                    &l2_slot_info,
                    false,
                    true,
                    operation_type,
                )
                .await
            {
                Ok(preconfed_block) => {
                    debug!(
                        "Preconfirmed forced inclusion L2 block: {:?}",
                        preconfed_block
                    );
                    preconfed_block
                }
                Err(err) => {
                    error!(
                        "Failed to advance head to new forced inclusion L2 block: {}",
                        err
                    );
                    return Err(anyhow::anyhow!(err));
                }
            };
            // set it to batch builder
            if !self
                .batch_builder
                .set_forced_inclusion(Some(forced_inclusion_batch))
            {
                error!("Failed to set forced inclusion to batch");
                return Err(anyhow::anyhow!("Failed to set forced inclusion to batch"));
            }
            Ok(preconfed_block)
        } else {
            error!("No forced inclusion to preconfirm in forced_inclusion");
            Err(anyhow::anyhow!(
                "No forced inclusion to preconfirm in forced_inclusion"
            ))
        }
    }

    async fn add_only_l2_block(
        &mut self,
        l2_block: L2Block,
        l2_slot_info: L2SlotInfo,
        end_of_sequencing: bool,
        operation_type: OperationType,
    ) -> Result<Option<BuildPreconfBlockResponse>, Error> {
        // insert l2 block into batch builder
        let anchor_block_id = self.consume_l2_block(l2_block.clone()).await?;

        match self
            .taiko
            .advance_head_to_new_l2_block(
                l2_block,
                anchor_block_id,
                &l2_slot_info,
                end_of_sequencing,
                false,
                operation_type,
            )
            .await
        {
            Ok(preconfed_block) => Ok(preconfed_block),
            Err(err) => {
                error!("Failed to advance head to new L2 block: {}", err);
                self.remove_last_l2_block();
                Ok(None)
            }
        }
    }

    async fn add_new_l2_block_with_optional_forced_inclusion(
        &mut self,
        l2_block: L2Block,
        l2_slot_info: L2SlotInfo,
        end_of_sequencing: bool,
        operation_type: OperationType,
        allow_forced_inclusion: bool,
    ) -> Result<
        (
            Option<BuildPreconfBlockResponse>,
            Option<BuildPreconfBlockResponse>,
        ),
        Error,
    > {
        // calculate the anchor block ID and create a new batch
        let anchor_block_id = self.calculate_anchor_block_id().await?;
        let anchor_block_timestamp_sec = self
            .ethereum_l1
            .execution_layer
            .get_block_timestamp_by_number(anchor_block_id)
            .await?;
        tracing::debug!(
            "Add new L2 block with optional forced inclusion: anchor_block_id: {}, anchor_block_timestamp_sec: {}, allow_forced_inclusion {}, !self.has_current_forced_inclusion(): {}",
            anchor_block_id,
            anchor_block_timestamp_sec,
            allow_forced_inclusion,
            !self.has_current_forced_inclusion(),
        );
        // Create new batch
        self.batch_builder
            .create_new_batch(anchor_block_id, anchor_block_timestamp_sec);

        if allow_forced_inclusion && !self.has_current_forced_inclusion() {
            let forced_inclusion_block_response = self
                .add_new_l2_block_with_forced_inclusion_when_needed(
                    &l2_slot_info,
                    operation_type,
                    anchor_block_id,
                )
                .await?;
            if forced_inclusion_block_response.is_some() {
                let (l2_block, l2_slot_info) = self.get_l2_block_after_forced_inclusion().await?;
                Ok((
                    forced_inclusion_block_response,
                    self.add_new_l2_block_to_new_batch(
                        l2_block,
                        l2_slot_info,
                        end_of_sequencing,
                        operation_type,
                    )
                    .await?,
                ))
            } else {
                Ok((None, None))
            }
        } else {
            Ok((
                None,
                self.add_new_l2_block_to_new_batch(
                    l2_block,
                    l2_slot_info,
                    end_of_sequencing,
                    operation_type,
                )
                .await?,
            ))
        }
    }

    async fn add_new_l2_block_to_new_batch(
        &mut self,
        l2_block: L2Block,
        l2_slot_info: L2SlotInfo,
        end_of_sequencing: bool,
        operation_type: OperationType,
    ) -> Result<Option<BuildPreconfBlockResponse>, Error> {
        // insert l2 block into batch builder
        let anchor_block_id = match self.consume_l2_block(l2_block.clone()).await {
            Ok(anchor_block_id) => anchor_block_id,
            Err(err) => {
                error!("Failed to consume L2 block: {}", err);
                self.batch_builder.remove_current_batch();
                return Ok(None);
            }
        };

        return match self
            .taiko
            .advance_head_to_new_l2_block(
                l2_block,
                anchor_block_id,
                &l2_slot_info,
                end_of_sequencing,
                false,
                operation_type,
            )
            .await
        {
            Ok(preconfed_block) => Ok(preconfed_block),
            Err(err) => {
                error!("Failed to advance head to new L2 block: {}", err);
                self.batch_builder.remove_current_batch();
                Ok(None)
            }
        };
    }

    async fn add_new_l2_block_with_forced_inclusion_when_needed(
        &mut self,
        l2_slot_info: &L2SlotInfo,
        operation_type: OperationType,
        anchor_block_id: u64,
    ) -> Result<Option<BuildPreconfBlockResponse>, Error> {
        // get current forced inclusion
        let start = std::time::Instant::now();
        let forced_inclusion = self.forced_inclusion.consume_forced_inclusion().await?;
        self.cached_forced_inclusion_txs = CachedForcedInclusion::Empty;
        debug!(
            "Got forced inclusion in {} milliseconds",
            start.elapsed().as_millis()
        );

        if let Some(forced_inclusion) = forced_inclusion {
            let forced_inclusion_batch = self
                .ethereum_l1
                .execution_layer
                .extension
                .build_forced_inclusion_batch(
                    self.batch_builder.get_config().default_coinbase,
                    anchor_block_id,
                    l2_slot_info.slot_timestamp(),
                    &forced_inclusion,
                );
            // preconfirm
            let forced_inclusion_block = L2Block {
                prebuilt_tx_list: PreBuiltTxList {
                    tx_list: forced_inclusion.txs,
                    estimated_gas_used: 0,
                    bytes_length: 0,
                },
                timestamp_sec: l2_slot_info.slot_timestamp(),
            };
            let forced_inclusion_block_response = match self
                .taiko
                .advance_head_to_new_l2_block(
                    forced_inclusion_block,
                    anchor_block_id,
                    l2_slot_info,
                    false,
                    true,
                    operation_type,
                )
                .await
            {
                Ok(preconfed_block) => {
                    debug!(
                        "Preconfirmed forced inclusion L2 block: {:?}",
                        preconfed_block
                    );
                    preconfed_block
                }
                Err(err) => {
                    error!(
                        "Failed to advance head to new forced inclusion L2 block: {}",
                        err
                    );
                    self.batch_builder.remove_current_batch();
                    return Ok(None); // TODO: why not return error here?
                }
            };
            // set it to batch builder
            if !self
                .batch_builder
                .set_forced_inclusion(Some(forced_inclusion_batch))
            {
                error!("Failed to set forced inclusion to batch");
                self.batch_builder.remove_current_batch();
                return Ok(None); // TODO: why not return error here?
            }
            return Ok(forced_inclusion_block_response);
        }

        Ok(None)
    }

    async fn get_l2_block_after_forced_inclusion(
        &mut self,
    ) -> Result<(L2Block, L2SlotInfo), Error> {
        // update slot info for next block
        let l2_slot_info = self
            .taiko
            .get_l2_slot_info_by_parent_block(alloy::eips::BlockNumberOrTag::Latest)
            .await?;
        // we need to update tx list because some txs might be in forced inclusion
        let pending_tx_list = match self
            .taiko
            .get_pending_l2_tx_list_from_taiko_geth(l2_slot_info.base_fee(), 0)
            .await?
        {
            Some(pending_tx_list) => pending_tx_list,
            None => {
                warn!(
                    "Failed to get pending tx list from taiko geth after forced inclusion. Add empty tx list"
                );
                PreBuiltTxList::empty()
            }
        };
        let l2_block = L2Block::new_from(pending_tx_list, l2_slot_info.slot_timestamp());
        info!(
            "Adding new L2 block after FI id: {}, timestamp: {}, parent gas used: {}, pending txs: {}",
            l2_slot_info.parent_id() + 1,
            l2_slot_info.slot_timestamp(),
            l2_slot_info.parent_gas_used(),
            l2_block.prebuilt_tx_list.tx_list.len(),
        );
        Ok((l2_block, l2_slot_info))
    }

    async fn add_new_l2_block(
        &mut self,
        l2_block: L2Block,
        l2_slot_info: L2SlotInfo,
        end_of_sequencing: bool,
        operation_type: OperationType,
        allow_forced_inclusion: bool,
    ) -> Result<
        (
            Option<BuildPreconfBlockResponse>,
            Option<BuildPreconfBlockResponse>,
        ),
        Error,
    > {
        info!(
            "Adding new L2 block id: {}, timestamp: {}, parent gas used: {}, allow_forced_inclusion: {}",
            l2_slot_info.parent_id() + 1,
            l2_slot_info.slot_timestamp(),
            l2_slot_info.parent_gas_used(),
            allow_forced_inclusion,
        );

        // Check that we will create a new batch
        if self.batch_builder.can_consume_l2_block(&l2_block) {
            let preconfed_block = self
                .add_only_l2_block(l2_block, l2_slot_info, end_of_sequencing, operation_type)
                .await?;
            Ok((None, preconfed_block))
        } else {
            self.add_new_l2_block_with_optional_forced_inclusion(
                l2_block,
                l2_slot_info,
                end_of_sequencing,
                operation_type,
                allow_forced_inclusion,
            )
            .await
        }
    }

    pub async fn consume_l2_block(&mut self, l2_block: L2Block) -> Result<u64, Error> {
        // If the L2 block can be added to the current batch, do so
        if self.batch_builder.can_consume_l2_block(&l2_block) {
            self.batch_builder
                .add_l2_block_and_get_current_anchor_block_id(l2_block)
        } else {
            // Otherwise, calculate the anchor block ID and create a new batch
            let anchor_block_id = self.calculate_anchor_block_id().await?;
            let anchor_block_timestamp_sec = self
                .ethereum_l1
                .execution_layer
                .get_block_timestamp_by_number(anchor_block_id)
                .await?;
            // Add the L2 block to the new batch
            self.batch_builder.create_new_batch_and_add_l2_block(
                anchor_block_id,
                anchor_block_timestamp_sec,
                l2_block,
                None,
            );
            Ok(anchor_block_id)
        }
    }

    fn remove_last_l2_block(&mut self) {
        self.batch_builder.remove_last_l2_block();
    }

    async fn calculate_anchor_block_id(&self) -> Result<u64, Error> {
        let height_from_last_batch = self
            .taiko
            .get_last_synced_anchor_block_id_from_taiko_anchor()
            .await?;
        let l1_height = self.ethereum_l1.execution_layer.get_l1_height().await?;
        let l1_height_with_lag = l1_height - self.l1_height_lag;
        let anchor_id_from_last_l2_block =
            match self.taiko.get_last_synced_anchor_block_id_from_geth().await {
                Ok(height) => height,
                Err(err) => {
                    warn!(
                        "Failed to get last anchor block ID from Taiko Geth: {:?}",
                        err
                    );
                    0
                }
            };

        Ok(std::cmp::max(
            std::cmp::max(height_from_last_batch, l1_height_with_lag),
            anchor_id_from_last_l2_block,
        ))
    }

    pub async fn try_submit_oldest_batch(
        &mut self,
        submit_only_full_batches: bool,
    ) -> Result<(), Error> {
        self.batch_builder
            .try_submit_oldest_batch(self.ethereum_l1.clone(), submit_only_full_batches)
            .await
    }

    pub fn has_batches(&self) -> bool {
        !self.batch_builder.is_empty()
    }

    pub fn has_current_forced_inclusion(&self) -> bool {
        self.batch_builder.has_current_forced_inclusion()
    }

    pub fn get_number_of_batches(&self) -> u64 {
        self.batch_builder.get_number_of_batches()
    }

    pub fn get_number_of_batches_ready_to_send(&self) -> u64 {
        self.batch_builder.get_number_of_batches_ready_to_send()
    }

    pub async fn reset_builder(&mut self) -> Result<(), Error> {
        warn!("Resetting batch builder");
        self.cached_forced_inclusion_txs = CachedForcedInclusion::Empty;
        self.forced_inclusion.sync_queue_index_with_head().await?;

        self.batch_builder = batch_builder::BatchBuilder::new(
            self.batch_builder.get_config().clone(),
            self.ethereum_l1.slot_clock.clone(),
            self.metrics.clone(),
        );

        Ok(())
    }

    pub fn clone_without_batches(&self) -> Self {
        Self {
            batch_builder: self.batch_builder.clone_without_batches(),
            ethereum_l1: self.ethereum_l1.clone(),
            taiko: self.taiko.clone(),
            l1_height_lag: self.l1_height_lag,
            forced_inclusion: self.forced_inclusion.clone(),
            cached_forced_inclusion_txs: CachedForcedInclusion::Empty,
            metrics: self.metrics.clone(),
        }
    }

    pub async fn update_forced_inclusion_and_clone_without_batches(
        &mut self,
    ) -> Result<Self, Error> {
        self.forced_inclusion.sync_queue_index_with_head().await?;
        Ok(self.clone_without_batches())
    }

    pub fn prepend_batches(&mut self, batches: BatchesToSend) {
        self.batch_builder.prepend_batches(batches);
    }

    pub fn try_finalize_current_batch(&mut self) -> Result<(), Error> {
        self.batch_builder.try_finalize_current_batch()
    }

    pub fn take_batches_to_send(&mut self) -> BatchesToSend {
        self.batch_builder.take_batches_to_send()
    }
}
