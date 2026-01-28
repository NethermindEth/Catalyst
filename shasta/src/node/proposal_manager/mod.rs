mod batch_builder;
pub mod l2_block_payload;
pub mod proposal;

use crate::{
    l1::execution_layer::ExecutionLayer,
    l2::taiko::Taiko,
    metrics::Metrics,
    shared::{l2_block_v2::L2BlockV2Draft, l2_tx_lists::PreBuiltTxList},
};
use alloy::{consensus::BlockHeader, consensus::Transaction};
use anyhow::Error;
use batch_builder::BatchBuilder;
use common::{batch_builder::BatchBuilderConfig, shared::l2_slot_info_v2::L2SlotContext};
use common::{
    l1::{ethereum_l1::EthereumL1, traits::ELTrait},
    l2::taiko_driver::{OperationType, models::BuildPreconfBlockResponse},
    shared::anchor_block_info::AnchorBlockInfo,
    utils::cancellation_token::CancellationToken,
};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::forced_inclusion::ForcedInclusion;
use crate::node::L2SlotInfoV2;
use proposal::Proposals;

const MIN_ANCHOR_OFFSET: u64 = 2;

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

    pub async fn try_submit_oldest_batch(
        &mut self,
        submit_only_full_batches: bool,
    ) -> Result<(), Error> {
        self.batch_builder
            .try_submit_oldest_batch(self.ethereum_l1.clone(), submit_only_full_batches)
            .await
    }

    pub fn should_new_block_be_created(
        &self,
        pending_tx_list: &Option<PreBuiltTxList>,
        l2_slot_context: &L2SlotContext,
    ) -> bool {
        self.batch_builder.should_new_block_be_created(
            pending_tx_list,
            l2_slot_context.info.slot_timestamp(),
            l2_slot_context.end_of_sequencing,
        )
    }

    pub async fn preconfirm_block(
        &mut self,
        pending_tx_list: Option<PreBuiltTxList>,
        l2_slot_context: &L2SlotContext,
    ) -> Result<BuildPreconfBlockResponse, Error> {
        let result = self
            .add_new_l2_block(
                pending_tx_list.unwrap_or_else(PreBuiltTxList::empty),
                l2_slot_context,
                OperationType::Preconfirm,
            )
            .await?;
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

    async fn add_new_l2_block_with_forced_inclusion_when_needed(
        &mut self,
        l2_slot_context: &L2SlotContext,
        operation_type: OperationType,
    ) -> Result<Option<BuildPreconfBlockResponse>, Error> {
        if self.has_current_forced_inclusion() {
            warn!("There is already a forced inclusion in the current batch");
            return Ok(None);
        }
        if !self.batch_builder.current_proposal_is_empty() {
            error!(
                "Cannot add new L2 block with forced inclusion because there are existing blocks in the current batch"
            );
            return Ok(None);
        }
        // get next forced inclusion
        let start = std::time::Instant::now();
        let forced_inclusion = self.forced_inclusion.consume_forced_inclusion().await?;
        debug!(
            "Checked forced inclusion in {} milliseconds",
            start.elapsed().as_millis()
        );

        if let Some(forced_inclusion) = forced_inclusion {
            let fi_block = L2BlockV2Draft {
                prebuilt_tx_list: PreBuiltTxList {
                    tx_list: forced_inclusion,
                    estimated_gas_used: 0,
                    bytes_length: 0,
                },
                timestamp_sec: l2_slot_context.info.parent_timestamp() + 1,
                gas_limit_without_anchor: l2_slot_context.info.parent_gas_limit_without_anchor(),
            };

            let anchor_params = self
                .taiko
                .l2_execution_layer()
                .get_last_synced_block_params_from_geth()
                .await?;

            let pyaload = self.batch_builder.add_fi_block(fi_block, anchor_params)?;
            match self
                .taiko
                .advance_head_to_new_l2_block(pyaload, l2_slot_context, operation_type)
                .await
            {
                Ok(fi_preconfed_block) => {
                    debug!(
                        "Preconfirmed forced inclusion L2 block: {:?}",
                        fi_preconfed_block
                    );
                    return Ok(Some(fi_preconfed_block));
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
        }

        Ok(None)
    }

    async fn add_new_l2_block(
        &mut self,
        prebuilt_tx_list: PreBuiltTxList,
        l2_slot_context: &L2SlotContext,
        operation_type: OperationType,
    ) -> Result<BuildPreconfBlockResponse, Error> {
        let timestamp = l2_slot_context.info.slot_timestamp();
        if let Some(last_block_timestamp) = self
            .batch_builder
            .get_current_proposal_last_block_timestamp()
            && timestamp == last_block_timestamp
        {
            return Err(anyhow::anyhow!(
                "Cannot add another block with the same timestamp as the last block, timestamp: {timestamp}, last block timestamp: {last_block_timestamp}"
            ));
        }

        info!(
            "Adding new L2 block id: {}, timestamp: {}, allow_forced_inclusion: {}",
            l2_slot_context.info.parent_id() + 1,
            timestamp,
            l2_slot_context.allow_forced_inclusion,
        );

        let l2_draft_block = L2BlockV2Draft {
            prebuilt_tx_list: prebuilt_tx_list.clone(),
            timestamp_sec: timestamp,
            gas_limit_without_anchor: l2_slot_context.info.parent_gas_limit_without_anchor(),
        };

        if !self.batch_builder.can_consume_l2_block(&l2_draft_block) {
            // Create new batch
            let _ = self.create_new_batch().await?;

            // Add forced inclusion when needed
            // not add forced inclusion when end_of_sequencing is true
            if l2_slot_context.allow_forced_inclusion
                && !l2_slot_context.end_of_sequencing
                && let Some(fi_block) = self
                    .add_new_l2_block_with_forced_inclusion_when_needed(
                        l2_slot_context,
                        operation_type,
                    )
                    .await?
            {
                return Ok(fi_block);
            }
        }

        let preconfed_block = self
            .add_draft_block_to_proposal(l2_draft_block, l2_slot_context, operation_type)
            .await?;

        Ok(preconfed_block)
    }

    async fn add_draft_block_to_proposal(
        &mut self,
        l2_draft_block: L2BlockV2Draft,
        l2_slot_context: &L2SlotContext,
        operation_type: OperationType,
    ) -> Result<BuildPreconfBlockResponse, Error> {
        let payload = self.batch_builder.add_l2_draft_block(l2_draft_block)?;

        match self
            .taiko
            .advance_head_to_new_l2_block(payload, l2_slot_context, operation_type)
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

    async fn get_next_proposal_id(&self) -> Result<u64, Error> {
        if let Some(current_proposal_id) = self.batch_builder.get_current_proposal_id() {
            return Ok(current_proposal_id + 1);
        }

        // Try fetching from L2 execution layer
        match self
            .taiko
            .l2_execution_layer()
            .get_last_synced_proposal_id_from_geth()
            .await
        {
            Ok(id) => Ok(id + 1),
            Err(_) => {
                // We can't retrieve the proposal ID from the latest L2 anchor block.
                // This can occur when there are no L2 blocks in Shasta yet.
                // Therefore, we verify it using the inbox state.
                warn!("Failed to get last synced proposal id from Taiko Geth");
                let inbox_state = self.ethereum_l1.execution_layer.get_inbox_state().await?;
                if inbox_state.nextProposalId == 1 {
                    Ok(1)
                } else {
                    Err(anyhow::anyhow!(
                        "Failed to get last synced proposal id from Taiko Geth, next_proposal_id = {}",
                        inbox_state.nextProposalId
                    ))
                }
            }
        }
    }

    async fn create_new_batch(&mut self) -> Result<u64, Error> {
        // Calculate the anchor block ID and create a new batch
        let last_anchor_id = self
            .taiko
            .l2_execution_layer()
            .get_last_synced_anchor_block_id_from_geth()
            .await
            .unwrap_or_else(|e| {
                warn!("Failed to get last synced anchor block ID from Taiko Geth: {e}");
                0
            });
        let anchor_block_info = AnchorBlockInfo::from_chain_state(
            self.ethereum_l1.execution_layer.common(),
            self.l1_height_lag,
            last_anchor_id,
            MIN_ANCHOR_OFFSET,
        )
        .await?;

        let proposal_id = self.get_next_proposal_id().await?;

        let anchor_block_id = anchor_block_info.id();
        // Create new batch
        self.batch_builder
            .create_new_batch(proposal_id, anchor_block_info);

        Ok(anchor_block_id)
    }

    fn remove_last_l2_block(&mut self) {
        self.batch_builder.remove_last_l2_block();
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

    pub fn has_batches(&self) -> bool {
        !self.batch_builder.is_empty()
    }

    pub fn has_current_forced_inclusion(&self) -> bool {
        self.batch_builder.has_current_forced_inclusion()
    }

    pub fn get_number_of_batches(&self) -> u64 {
        self.batch_builder.get_number_of_batches()
    }

    pub fn try_finalize_current_batch(&mut self) -> Result<(), Error> {
        self.batch_builder.try_finalize_current_batch()
    }

    pub fn take_batches_to_send(&mut self) -> Proposals {
        self.batch_builder.take_batches_to_send()
    }

    pub fn is_anchor_block_offset_valid(&self, anchor_block_offset: u64) -> bool {
        anchor_block_offset
            < self
                .taiko
                .get_protocol_config()
                .get_max_anchor_height_offset()
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
        let l1_anchor_block_id = Taiko::decode_anchor_id_from_tx_data(l2_anchor_tx.input())?;

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

    pub async fn recover_from_l2_block(&mut self, block_height: u64) -> Result<(), Error> {
        debug!("Recovering from L2 block {}", block_height);
        let block = self
            .taiko
            .get_l2_block_by_number(block_height, true)
            .await?;
        let (anchor_tx, txs) = match block.transactions.as_transactions() {
            Some(txs) => txs.split_first().ok_or_else(|| {
                anyhow::anyhow!("recover_from_l2_block: Cannot get anchor transaction from block")
            })?,
            None => {
                return Err(anyhow::anyhow!(
                    "recover_from_l2_block: No transactions in block"
                ));
            }
        };

        use taiko_alethia_reth::validation::ANCHOR_V3_V4_GAS_LIMIT;
        let gas_limit = block
            .header
            .gas_limit()
            .checked_sub(ANCHOR_V3_V4_GAS_LIMIT)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "block header gas limit {} is less than ANCHOR_V3_V4_GAS_LIMIT {}",
                    block.header.gas_limit(),
                    ANCHOR_V3_V4_GAS_LIMIT
                )
            })?;

        let coinbase = block.header.beneficiary();

        let proposal_id =
            crate::l2::extra_data::ExtraData::decode(block.header.extra_data())?.proposal_id;

        let anchor_tx_data = Taiko::get_anchor_tx_data(anchor_tx.input())?;
        let anchor_info = AnchorBlockInfo::from_precomputed_data(
            self.ethereum_l1.execution_layer.common(),
            anchor_tx_data._checkpoint.blockNumber.to::<u64>(),
            anchor_tx_data._checkpoint.blockHash,
            anchor_tx_data._checkpoint.stateRoot,
        )
        .await?;

        let is_forced_inclusion = self.is_forced_inclusion(block_height).await?;

        // TODO improve output
        debug!(
            "Recovering from L2 block {}, proposal_id: {} transactions: {} is_forced_inclusion: {}, timestamp: {}, anchor_block_number: {} coinbase: {}, gas_limit: {}",
            block_height,
            proposal_id,
            txs.len(),
            is_forced_inclusion,
            block.header.timestamp(),
            anchor_info.id(),
            coinbase,
            gas_limit
        );

        let txs = txs.to_vec();

        // TODO validate block params
        self.batch_builder
            .recover_from(
                proposal_id,
                anchor_info,
                coinbase,
                txs,
                block.header.timestamp(),
                gas_limit,
                is_forced_inclusion,
            )
            .await?;
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
        // TODO handle forced inclusion properly
        //self.forced_inclusion.sync_queue_index_with_head().await?;
        Ok(self.clone_without_batches())
    }

    pub fn prepend_batches(&mut self, batches: Proposals) {
        self.batch_builder.prepend_batches(batches);
    }

    pub async fn reanchor_block(
        &mut self,
        pending_tx_list: PreBuiltTxList,
        l2_slot_info: L2SlotInfoV2,
        _is_forced_inclusion: bool,
        allow_forced_inclusion: bool,
    ) -> Result<BuildPreconfBlockResponse, Error> {
        // TODO create context outside
        let l2_slot_context = L2SlotContext {
            info: l2_slot_info,
            end_of_sequencing: false,
            allow_forced_inclusion,
        };

        // TODO handle forced inclusion properly

        let block = self
            .add_new_l2_block(pending_tx_list, &l2_slot_context, OperationType::Reanchor)
            .await?;

        Ok(block)
    }

    pub async fn is_forced_inclusion(&mut self, block_id: u64) -> Result<bool, Error> {
        let is_forced_inclusion = self
            .taiko
            .get_forced_inclusion_form_l1origin(block_id)
            .await
            .map_err(|e| {
                anyhow::anyhow!("Failed to get forced inclusion flag from Taiko Geth: {e}")
            })?;

        Ok(is_forced_inclusion)
    }
}
