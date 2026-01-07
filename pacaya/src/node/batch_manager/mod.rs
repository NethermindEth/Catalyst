pub mod batch;
mod batch_builder;
pub mod config;

use crate::{
    forced_inclusion::ForcedInclusion,
    l1::execution_layer::ExecutionLayer,
    l2::{
        self,
        taiko::{self, Taiko},
    },
    metrics::Metrics,
    node::batch_manager::config::BatchesToSend,
    shared::{l2_block::L2Block, l2_slot_info::L2SlotInfo, l2_tx_lists::PreBuiltTxList},
};
use alloy::{consensus::BlockHeader, consensus::Transaction, primitives::Address};
use anyhow::Error;
use batch_builder::BatchBuilder;
use common::{
    batch_builder::BatchBuilderConfig,
    l1::{ethereum_l1::EthereumL1, traits::ELTrait},
    l2::taiko_driver::{OperationType, models::BuildPreconfBlockResponse},
    utils::cancellation_token::CancellationToken,
};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

pub struct BatchManager {
    batch_builder: BatchBuilder,
    ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
    pub taiko: Arc<Taiko>,
    l1_height_lag: u64,
    forced_inclusion: Arc<ForcedInclusion>,
    metrics: Arc<Metrics>,
    cancel_token: CancellationToken,
}

impl BatchManager {
    pub async fn new(
        l1_height_lag: u64,
        config: BatchBuilderConfig,
        ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
        taiko: Arc<Taiko>,
        metrics: Arc<Metrics>,
        cancel_token: CancellationToken,
    ) -> Result<Self, Error> {
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
        let forced_inclusion = Arc::new(ForcedInclusion::new(ethereum_l1.clone()).await?);
        Ok(Self {
            batch_builder: BatchBuilder::new(
                config,
                ethereum_l1.slot_clock.clone(),
                metrics.clone(),
            ),
            ethereum_l1,
            taiko,
            l1_height_lag,
            forced_inclusion,
            metrics,
            cancel_token,
        })
    }

    pub async fn is_forced_inclusion(&mut self, block_id: u64) -> Result<bool, Error> {
        let is_forced_inclusion = match self
            .taiko
            .get_forced_inclusion_form_l1origin(block_id)
            .await
        {
            Ok(fi) => fi,
            Err(e) => {
                error!("Failed to get forced inclusion flag from Taiko Geth: {e}");
                return Err(anyhow::anyhow!(
                    "Failed to get forced inclusion flag from Taiko Geth: {e}"
                ));
            }
        };

        Ok(is_forced_inclusion)
    }

    pub async fn check_and_handle_forced_inclusion(
        &mut self,
        block_id: u64,
        coinbase: Address,
        anchor_block_id: u64,
        timestamp: u64,
    ) -> Result<bool, Error> {
        let forced_inclusion = self.is_forced_inclusion(block_id).await?;
        debug!(
            "Handle forced inclusion: is forced inclusion: {}",
            forced_inclusion
        );

        if forced_inclusion {
            self.batch_builder.try_finalize_current_batch()?;
            let forced_inclusion = self.forced_inclusion.consume_forced_inclusion().await?;
            if let Some(forced_inclusion) = forced_inclusion {
                let forced_inclusion_batch = self
                    .ethereum_l1
                    .execution_layer
                    .build_forced_inclusion_batch(
                        coinbase,
                        anchor_block_id,
                        timestamp,
                        &forced_inclusion,
                    );
                // set it to batch builder
                if !self
                    .batch_builder
                    .set_forced_inclusion(forced_inclusion_batch)
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
            .common()
            .get_block_timestamp_by_number(anchor_block_id)
            .await?;

        let txs = txs.to_vec();
        let forced_inclusion_handled = self
            .check_and_handle_forced_inclusion(
                block_height,
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
                .common()
                .get_block_timestamp_by_number(l1_anchor_block_id)
                .await?,
        )
    }

    pub fn is_anchor_block_offset_valid(&self, anchor_block_offset: u64) -> bool {
        anchor_block_offset
            < self
                .taiko
                .get_protocol_config()
                .get_config_max_anchor_height_offset()
    }

    pub async fn reanchor_block(
        &mut self,
        pending_tx_list: PreBuiltTxList,
        l2_slot_info: &L2SlotInfo,
        is_forced_inclusion: bool,
        allow_forced_inclusion: bool,
    ) -> Result<BuildPreconfBlockResponse, Error> {
        let l2_block = L2Block::new_from(pending_tx_list, l2_slot_info.slot_timestamp());

        if is_forced_inclusion && allow_forced_inclusion {
            return Err(anyhow::anyhow!(
                "Skip forced inclusion block because we had OldestForcedInclusionDue"
            ));
        }

        let block = if is_forced_inclusion {
            self.preconfirm_forced_inclusion_block(l2_slot_info, OperationType::Reanchor)
                .await?
        } else {
            self.add_new_l2_block(
                l2_block,
                l2_slot_info,
                false,
                OperationType::Reanchor,
                allow_forced_inclusion,
            )
            .await?
        };

        Ok(block)
    }

    pub async fn preconfirm_block(
        &mut self,
        pending_tx_list: Option<PreBuiltTxList>,
        l2_slot_info: &L2SlotInfo,
        end_of_sequencing: bool,
        allow_forced_inclusion: bool,
    ) -> Result<Option<BuildPreconfBlockResponse>, Error> {
        let result = if let Some(l2_block) = self.batch_builder.try_creating_l2_block(
            pending_tx_list,
            l2_slot_info.slot_timestamp(),
            end_of_sequencing,
        ) {
            Some(
                self.add_new_l2_block(
                    l2_block,
                    l2_slot_info,
                    end_of_sequencing,
                    OperationType::Preconfirm,
                    allow_forced_inclusion,
                )
                .await?,
            )
        } else {
            None
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
        l2_slot_info: &L2SlotInfo,
        operation_type: OperationType,
    ) -> Result<BuildPreconfBlockResponse, Error> {
        let anchor_block_id = self.calculate_anchor_block_id().await?;

        let start = std::time::Instant::now();
        let forced_inclusion = self.forced_inclusion.consume_forced_inclusion().await?;
        debug!(
            "Got forced inclusion in {} milliseconds",
            start.elapsed().as_millis()
        );

        if let Some(forced_inclusion) = forced_inclusion {
            let forced_inclusion_batch = self
                .ethereum_l1
                .execution_layer
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
                    self.ethereum_l1
                        .execution_layer
                        .common()
                        .get_block_state_root_by_number(anchor_block_id)
                        .await?,
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
                    return Err(anyhow::anyhow!(err));
                }
            };
            // set it to batch builder
            if !self
                .batch_builder
                .set_forced_inclusion(forced_inclusion_batch)
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

    async fn add_new_l2_block_with_forced_inclusion_when_needed(
        &mut self,
        l2_slot_info: &L2SlotInfo,
        operation_type: OperationType,
        anchor_block_id: u64,
    ) -> Result<Option<BuildPreconfBlockResponse>, Error> {
        if self.has_current_forced_inclusion() {
            warn!("There is already a forced inclusion in the current batch");
            return Ok(None);
        }
        if !self.batch_builder.current_batch_is_empty() {
            error!(
                "Cannot add new L2 block with forced inclusion because there are existing blocks in the current batch"
            );
            return Ok(None);
        }
        // get next forced inclusion
        let start = std::time::Instant::now();
        let forced_inclusion = self.forced_inclusion.consume_forced_inclusion().await?;
        debug!(
            "Got forced inclusion in {} milliseconds",
            start.elapsed().as_millis()
        );

        if let Some(forced_inclusion) = forced_inclusion {
            let forced_inclusion_batch = self
                .ethereum_l1
                .execution_layer
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
                    self.ethereum_l1
                        .execution_layer
                        .common()
                        .get_block_state_root_by_number(anchor_block_id)
                        .await?,
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
                    self.forced_inclusion.release_forced_inclusion().await;
                    self.batch_builder.remove_current_batch();
                    return Err(anyhow::anyhow!(
                        "Failed to advance head to new forced inclusion L2 block: {}",
                        err
                    ));
                }
            };
            // set it to batch builder
            if !self
                .batch_builder
                .set_forced_inclusion(forced_inclusion_batch)
            {
                // We should never enter here because it means we already have a forced inclusion
                // but we didn't set it yet. And at the beginning of the function we checked if
                // the forced inclusion is empty. This is a bug in the code logic
                error!("Failed to set forced inclusion to batch");
                self.cancel_token.cancel_on_critical_error();
                return Err(anyhow::anyhow!("Failed to set forced inclusion to batch"));
            }
            return Ok(Some(forced_inclusion_block_response));
        }

        Ok(None)
    }

    async fn add_new_l2_block_to_batch(
        &mut self,
        l2_block: L2Block,
        l2_slot_info: &L2SlotInfo,
        end_of_sequencing: bool,
        operation_type: OperationType,
    ) -> Result<BuildPreconfBlockResponse, Error> {
        let anchor_block_id = self
            .batch_builder
            .add_l2_block_and_get_current_anchor_block_id(l2_block.clone())?;

        match self
            .taiko
            .advance_head_to_new_l2_block(
                l2_block,
                anchor_block_id,
                self.ethereum_l1
                    .execution_layer
                    .common()
                    .get_block_state_root_by_number(anchor_block_id)
                    .await?,
                l2_slot_info,
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
                Err(anyhow::anyhow!(
                    "Failed to advance head to new L2 block: {}",
                    err
                ))
            }
        }
    }

    async fn create_new_batch(&mut self) -> Result<u64, Error> {
        // Calculate the anchor block ID and create a new batch
        let anchor_block_id = self.calculate_anchor_block_id().await?;
        let anchor_block_timestamp_sec = self
            .ethereum_l1
            .execution_layer
            .common()
            .get_block_timestamp_by_number(anchor_block_id)
            .await?;

        // Create new batch
        self.batch_builder
            .create_new_batch(anchor_block_id, anchor_block_timestamp_sec);

        Ok(anchor_block_id)
    }

    pub async fn add_new_l2_block_with_forced_inclusion(
        &mut self,
        operation_type: OperationType,
        l2_slot_info: &L2SlotInfo,
    ) -> Result<Option<BuildPreconfBlockResponse>, Error> {
        // TODO Should we use try here?
        let anchor_block_id = self.create_new_batch().await?;

        self.add_new_l2_block_with_forced_inclusion_when_needed(
            l2_slot_info,
            operation_type,
            anchor_block_id,
        )
        .await
    }

    async fn add_new_l2_block(
        &mut self,
        l2_block: L2Block,
        l2_slot_info: &L2SlotInfo,
        end_of_sequencing: bool,
        operation_type: OperationType,
        allow_forced_inclusion: bool,
    ) -> Result<BuildPreconfBlockResponse, Error> {
        info!(
            "Adding new L2 block id: {}, timestamp: {}, parent gas used: {}, allow_forced_inclusion: {}",
            l2_slot_info.parent_id() + 1,
            l2_slot_info.slot_timestamp(),
            l2_slot_info.parent_gas_used(),
            allow_forced_inclusion,
        );

        if !self.batch_builder.can_consume_l2_block(&l2_block) {
            // Create new batch
            let anchor_block_id = self.create_new_batch().await?;

            // Add forced inclusion when needed
            // not add forced inclusion when end_of_sequencing is true
            if allow_forced_inclusion
                && !end_of_sequencing
                && let Some(fi_block) = self
                    .add_new_l2_block_with_forced_inclusion_when_needed(
                        l2_slot_info,
                        operation_type,
                        anchor_block_id,
                    )
                    .await?
            {
                return Ok(fi_block);
            }
        }

        self.add_new_l2_block_to_batch(l2_block, l2_slot_info, end_of_sequencing, operation_type)
            .await
    }

    fn remove_last_l2_block(&mut self) {
        self.batch_builder.remove_last_l2_block();
    }

    async fn calculate_anchor_block_id(&self) -> Result<u64, Error> {
        let height_from_last_batch = self
            .taiko
            .get_last_synced_anchor_block_id_from_taiko_anchor()
            .await?;
        let l1_height = self
            .ethereum_l1
            .execution_layer
            .common()
            .get_latest_block_id()
            .await?;
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
            metrics: self.metrics.clone(),
            cancel_token: self.cancel_token.clone(),
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
