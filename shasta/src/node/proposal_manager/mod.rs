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
    forced_inclusion: ForcedInclusion,
    metrics: Arc<Metrics>,
    cancel_token: CancellationToken,
    max_blocks_to_reanchor: u64,
    propose_forced_inclusion: bool,
}

impl BatchManager {
    pub async fn new(
        l1_height_lag: u64,
        config: BatchBuilderConfig,
        ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
        taiko: Arc<Taiko>,
        metrics: Arc<Metrics>,
        cancel_token: CancellationToken,
        max_blocks_to_reanchor: u64,
        propose_forced_inclusion: bool,
    ) -> Result<Self, Error> {
        info!(
            "Batch builder config:\n\
             max_bytes_size_of_batch: {}\n\
             max_blocks_per_batch: {}\n\
             l1_slot_duration_sec: {}\n\
             max_time_shift_between_blocks_sec: {}\n\
             max_anchor_height_offset: {}\n\
             proposal_max_time_sec: {}",
            config.max_bytes_size_of_batch,
            config.max_blocks_per_batch,
            config.l1_slot_duration_sec,
            config.max_time_shift_between_blocks_sec,
            config.max_anchor_height_offset,
            config.proposal_max_time_sec,
        );

        let forced_inclusion = ForcedInclusion::new(ethereum_l1.clone()).await?;

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
            max_blocks_to_reanchor,
            propose_forced_inclusion,
        })
    }

    pub async fn try_submit_oldest_batch(
        &mut self,
        submit_only_full_batches: bool,
        l2_slot_timestamp: u64,
    ) -> Result<(), Error> {
        self.batch_builder
            .try_submit_oldest_batch(
                self.ethereum_l1.clone(),
                submit_only_full_batches,
                l2_slot_timestamp,
            )
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
        let (result, _) = self
            .add_new_l2_block(
                pending_tx_list.unwrap_or_else(PreBuiltTxList::empty),
                l2_slot_context,
                OperationType::Preconfirm,
                true,
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
        if !self.batch_builder.can_add_forced_inclusion() {
            return Ok(None);
        }
        // get next forced inclusion
        let forced_inclusion = self.forced_inclusion.consume_forced_inclusion().await?;

        if let Some(forced_inclusion) = forced_inclusion {
            debug!(
                "⏺️ Adding new forced inclusion block with {} transactions",
                forced_inclusion.len()
            );
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
                .get_block_params_from_geth(l2_slot_context.info.parent_id())
                .await?;

            let payload = self.batch_builder.add_fi_block(fi_block, anchor_params)?;
            match self
                .taiko
                .advance_head_to_new_l2_block(payload, l2_slot_context, operation_type)
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
                    self.batch_builder.decrease_forced_inclusion_count();
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
        allow_forced_inclusion: bool,
        // TODO REPLACE with enum or struct
    ) -> Result<(BuildPreconfBlockResponse, bool), Error> {
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

        let allow_forced_inclusion = self.propose_forced_inclusion
            && allow_forced_inclusion
            && !l2_slot_context.end_of_sequencing;
        info!(
            "Adding new L2 block id: {}, timestamp: {}, allow_forced_inclusion: {}",
            l2_slot_context.info.parent_id() + 1,
            timestamp,
            allow_forced_inclusion,
        );

        let l2_draft_block = L2BlockV2Draft {
            prebuilt_tx_list: prebuilt_tx_list.clone(),
            timestamp_sec: timestamp,
            gas_limit_without_anchor: l2_slot_context.info.parent_gas_limit_without_anchor(),
        };

        if !self.batch_builder.can_consume_l2_block(&l2_draft_block) {
            // Create new batch
            let _ = self
                .create_new_batch(
                    l2_slot_context.info.parent_id(),
                    l2_slot_context.info.slot_timestamp(),
                )
                .await?;
        }

        // Add forced inclusion when needed
        if allow_forced_inclusion
            && let Some(fi_block) = self
                .add_new_l2_block_with_forced_inclusion_when_needed(l2_slot_context, operation_type)
                .await?
        {
            return Ok((fi_block, true));
        }

        let preconfed_block = self
            .add_draft_block_to_proposal(l2_draft_block, l2_slot_context, operation_type)
            .await?;

        Ok((preconfed_block, false))
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

    async fn get_next_proposal_id(&self, parent_block_id: u64) -> Result<u64, Error> {
        if let Some(current_proposal_id) = self.batch_builder.get_current_proposal_id() {
            return Ok(current_proposal_id + 1);
        }

        // Try fetching from L2 execution layer
        match self
            .taiko
            .l2_execution_layer()
            .get_proposal_id_from_geth_by_block_id(parent_block_id)
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

    async fn create_new_batch(
        &mut self,
        parent_block_id: u64,
        l2_slot_timestamp: u64,
    ) -> Result<u64, Error> {
        // Calculate the anchor block ID and create a new batch
        let last_anchor_id = self
            .taiko
            .l2_execution_layer()
            .get_anchor_block_id_from_geth(parent_block_id)
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

        let proposal_id = self.get_next_proposal_id(parent_block_id).await?;

        let anchor_block_id = anchor_block_info.id();
        // Create new batch
        self.batch_builder
            .create_new_batch(proposal_id, anchor_block_info, l2_slot_timestamp);

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

    pub fn is_offsets_valid(&self, anchor_block_offset: u64, timestamp_offset: u64) -> bool {
        self.is_anchor_block_offset_valid(anchor_block_offset)
            && self.is_timestamp_offset_valid(timestamp_offset)
    }

    fn is_anchor_block_offset_valid(&self, anchor_block_offset: u64) -> bool {
        anchor_block_offset
            <= self
                .taiko
                .get_protocol_config()
                .get_max_anchor_height_offset()
    }

    fn is_timestamp_offset_valid(&self, timestamp_offset: u64) -> bool {
        timestamp_offset <= self.taiko.get_protocol_config().get_timestamp_max_offset()
    }

    pub async fn get_l1_anchor_block_and_timestamp_offset_for_l2_block(
        &self,
        l2_block_height: u64,
    ) -> Result<(u64, u64), Error> {
        debug!(
            "get_anchor_block_offset: Checking L2 block {}",
            l2_block_height
        );
        let block = self
            .taiko
            .get_l2_block_by_number(l2_block_height, false)
            .await?;
        let block_timestamp = block.header.timestamp();

        let anchor_tx_hash = block
            .transactions
            .as_hashes()
            .and_then(|txs| txs.first())
            .ok_or_else(|| anyhow::anyhow!("get_anchor_block_offset: No transactions in block"))?;

        let l2_anchor_tx = self.taiko.get_transaction_by_hash(*anchor_tx_hash).await?;
        let l1_anchor_block_id = Taiko::decode_anchor_id_from_tx_data(l2_anchor_tx.input())?;

        debug!(
            "get_l1_anchor_block_and_timestamp_offset_for_l2_block: L2 block {l2_block_height} has L1 anchor block id {l1_anchor_block_id} and  timestamp {block_timestamp}",
        );

        let anchor_offset = self.ethereum_l1.slot_clock.slots_since_l1_block(
            self.ethereum_l1
                .execution_layer
                .common()
                .get_block_timestamp_by_number(l1_anchor_block_id)
                .await?,
        )?;
        let timestamp_offset = self.ethereum_l1.slot_clock.seconds_since(block_timestamp);
        Ok((anchor_offset, timestamp_offset))
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

    pub fn clone_without_batches(&self, fi_head: u64) -> Self {
        Self {
            batch_builder: self.batch_builder.clone_without_batches(),
            ethereum_l1: self.ethereum_l1.clone(),
            taiko: self.taiko.clone(),
            l1_height_lag: self.l1_height_lag,
            forced_inclusion: ForcedInclusion::new_with_index(self.ethereum_l1.clone(), fi_head),
            metrics: self.metrics.clone(),
            cancel_token: self.cancel_token.clone(),
            max_blocks_to_reanchor: self.max_blocks_to_reanchor,
            propose_forced_inclusion: self.propose_forced_inclusion,
        }
    }

    pub fn prepend_batches(&mut self, batches: Proposals) {
        self.batch_builder.prepend_batches(batches);
    }

    pub fn set_fi_head(&mut self, fi_head: u64) {
        self.forced_inclusion.set_index(fi_head);
    }

    async fn reanchor_block(
        &mut self,
        pending_tx_list: PreBuiltTxList,
        l2_slot_info: L2SlotInfoV2,
        allow_forced_inclusion: bool,
        // TODO REPLACE with enum or struct
    ) -> Result<(BuildPreconfBlockResponse, bool), Error> {
        let l2_slot_context = L2SlotContext {
            info: l2_slot_info,
            end_of_sequencing: false,
        };

        let (block, is_forced_inclusion) = self
            .add_new_l2_block(
                pending_tx_list,
                &l2_slot_context,
                OperationType::Reanchor,
                allow_forced_inclusion,
            )
            .await?;

        Ok((block, is_forced_inclusion))
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

    pub async fn reanchor_blocks(
        &mut self,
        blocks: &[alloy::rpc::types::Block],
        forced_inclusion_flags: &[bool],
        parent_block_id: u64,
    ) -> Result<u64, Error> {
        let mut current_block_pos = 0;
        let mut processed_blocks = 0;
        let mut is_common_block_processed = false;

        // calculate slot info for the first block
        let (first_l2_slot_info, max_blocks_to_reanchor) =
            self.prepare_reanchor_slot_info(parent_block_id).await?;

        while current_block_pos < blocks.len() && processed_blocks < max_blocks_to_reanchor {
            debug!(
                "Reanchoring block position {}/{}, processed: {}/{}",
                current_block_pos,
                blocks.len(),
                processed_blocks,
                max_blocks_to_reanchor
            );

            if forced_inclusion_flags[current_block_pos] {
                debug!(
                    "Skipping forced inclusion block {}",
                    blocks[current_block_pos].header.number,
                );
                current_block_pos += 1;
                continue;
            }

            let block = &blocks[current_block_pos];
            let txs = self.extract_block_transactions(block)?;

            // Skip empty blocks, except the first one
            if txs.is_empty() && is_common_block_processed {
                debug!("Skipping empty block {}", block.header.number);
                current_block_pos += 1;
                continue;
            }

            let l2_slot_info = self
                .get_l2_slot_info_for_reanchor(&first_l2_slot_info, processed_blocks)
                .await?;
            debug!(
                "Reanchoring block {} with {} txs, parent: {}, timestamp: {}",
                block.header.number,
                txs.len(),
                l2_slot_info.parent_id(),
                l2_slot_info.slot_timestamp(),
            );

            let pending_tx_list = PreBuiltTxList {
                tx_list: txs,
                estimated_gas_used: 0,
                bytes_length: 0,
            };

            let is_last_reanchored_block = current_block_pos + 1 == blocks.len()
                || processed_blocks + 1 == max_blocks_to_reanchor;
            let allow_forced_inclusion = !is_last_reanchored_block;

            match self
                .reanchor_block(pending_tx_list, l2_slot_info, allow_forced_inclusion)
                .await
            {
                Ok((reanchored_block, is_forced_inclusion)) => {
                    debug!(
                        "Reanchored block {} hash {}, is_forced_inclusion: {}",
                        reanchored_block.number, reanchored_block.hash, is_forced_inclusion,
                    );
                    processed_blocks += 1;
                    if !is_forced_inclusion {
                        is_common_block_processed = true;
                        current_block_pos += 1;
                    }
                }
                Err(err) => {
                    error!("Failed to reanchor block {}: {}", block.header.number, err);
                    self.cancel_token.cancel_on_critical_error();
                    return Err(anyhow::anyhow!(
                        "Failed to reanchor block {}: {}",
                        block.header.number,
                        err
                    ));
                }
            }
        }
        // finalize the current batch to avoid anchor and timestamp checks during preconfirmation
        self.try_finalize_current_batch()?;
        Ok(processed_blocks)
    }

    async fn prepare_reanchor_slot_info(
        &self,
        parent_block_id: u64,
    ) -> Result<(L2SlotInfoV2, u64), Error> {
        let info = self
            .taiko
            .get_l2_slot_info_by_parent_block(alloy::eips::BlockNumberOrTag::Number(
                parent_block_id,
            ))
            .await?;
        let max_blocks_to_reanchor =
            (self.max_blocks_to_reanchor).min(info.slot_timestamp() - info.parent_timestamp());
        let first_block_timestamp = info.slot_timestamp() - max_blocks_to_reanchor;
        let l2_slot_info = L2SlotInfoV2::new_from_other(info, first_block_timestamp);
        Ok((l2_slot_info, max_blocks_to_reanchor))
    }

    async fn get_l2_slot_info_for_reanchor(
        &self,
        first_slot_info: &L2SlotInfoV2,
        processed_blocks: u64,
    ) -> Result<L2SlotInfoV2, Error> {
        if processed_blocks == 0 {
            Ok(first_slot_info.clone())
        } else {
            let info = self.taiko.get_l2_slot_info().await?;
            let timestamp = info.parent_timestamp() + 1;
            Ok(L2SlotInfoV2::new_from_other(info, timestamp))
        }
    }

    fn extract_block_transactions(
        &self,
        block: &alloy::rpc::types::Block,
    ) -> Result<Vec<alloy::rpc::types::Transaction>, Error> {
        let (_, txs) = block
            .transactions
            .as_transactions()
            .and_then(|txs| txs.split_first())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Cannot extract transactions from block {}",
                    block.header.number
                )
            })?;
        Ok(txs.to_vec())
    }
}
