mod batch_builder;
pub mod proposal;

use crate::{
    l1::execution_layer::ExecutionLayer,
    l2::taiko::Taiko,
    metrics::Metrics,
    shared::{l2_block::L2Block, l2_slot_info::L2SlotInfo, l2_tx_lists::PreBuiltTxList},
};
use alloy::{consensus::BlockHeader, consensus::Transaction};
use anyhow::Error;
use batch_builder::BatchBuilder;
use common::batch_builder::BatchBuilderConfig;
use common::{
    l1::{ethereum_l1::EthereumL1, traits::ELTrait},
    l2::taiko_driver::{OperationType, models::BuildPreconfBlockResponse},
    shared::anchor_block_info::AnchorBlockInfo,
};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::node::proposal_manager::proposal::BondInstructionData;
use alloy::primitives::{B256, U256};
use proposal::Proposals;
use taiko_protocol::shasta::constants::BOND_PROCESSING_DELAY;

const MIN_ANCHOR_OFFSET: u64 = 2;

pub struct BatchManager {
    batch_builder: BatchBuilder,
    ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
    pub taiko: Arc<Taiko>,
    l1_height_lag: u64,
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

        Ok(Self {
            batch_builder: BatchBuilder::new(
                config,
                ethereum_l1.slot_clock.clone(),
                metrics.clone(),
            ),
            ethereum_l1,
            taiko,
            l1_height_lag,
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
            self.add_new_l2_block(
                l2_block,
                l2_slot_info,
                end_of_sequencing,
                OperationType::Preconfirm,
                allow_forced_inclusion,
            )
            .await?
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

    async fn add_new_l2_block(
        &mut self,
        l2_block: L2Block,
        l2_slot_info: &L2SlotInfo,
        end_of_sequencing: bool,
        operation_type: OperationType,
        allow_forced_inclusion: bool,
    ) -> Result<Option<BuildPreconfBlockResponse>, Error> {
        info!(
            "Adding new L2 block id: {}, timestamp: {}, parent gas used: {}, allow_forced_inclusion: {}",
            l2_slot_info.parent_id() + 1,
            l2_slot_info.slot_timestamp(),
            l2_slot_info.parent_gas_used(),
            allow_forced_inclusion,
        );

        if !self.batch_builder.can_consume_l2_block(&l2_block) {
            // Create new batch
            let _anchor_block_id = self.create_new_batch().await?;

            // Add forced inclusion when needed
            // not add forced inclusion when end_of_sequencing is true
            // TODO add fi blocks
        }

        let preconfed_block = self
            .add_new_l2_block_to_batch(l2_block, l2_slot_info, end_of_sequencing, operation_type)
            .await?;

        Ok(preconfed_block)
    }

    async fn add_new_l2_block_to_batch(
        &mut self,
        l2_block: L2Block,
        l2_slot_info: &L2SlotInfo,
        end_of_sequencing: bool,
        operation_type: OperationType,
    ) -> Result<Option<BuildPreconfBlockResponse>, Error> {
        let proposal = self
            .batch_builder
            .add_l2_block_and_get_current_proposal(l2_block.clone())?;

        match self
            .taiko
            .advance_head_to_new_l2_block(
                proposal,
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

    async fn get_bond_instructions(&self, proposal_id: u64) -> Result<BondInstructionData, Error> {
        if proposal_id <= BOND_PROCESSING_DELAY {
            return Ok(BondInstructionData::new(Vec::new(), B256::ZERO));
        }

        // Calculate the proposal ID to query, adjusting for processing delay
        let target_id = proposal_id.saturating_sub(BOND_PROCESSING_DELAY);

        // Fetch the proposal payload from the event indexer
        let target_payload = self
            .ethereum_l1
            .execution_layer
            .event_indexer
            .get_indexer()
            .get_proposal_by_id(U256::from(target_id))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Can't get bond instruction from event indexer (id: {})",
                    target_id
                )
            })?;

        // Extract the bond instructions hash
        let target_hash =
            B256::from_slice(target_payload.core_state.bondInstructionsHash.as_slice());

        Ok(BondInstructionData::new(
            target_payload.bond_instructions,
            target_hash,
        ))
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
            // If fetching from L2 fails (e.g., no blocks in Shasta), fallback to event indexer
            Err(_) => self.get_proposal_id_from_indexer_fallback().await,
        }
    }

    async fn get_proposal_id_from_indexer_fallback(&self) -> Result<u64, Error> {
        let id = self
            .ethereum_l1
            .execution_layer
            .get_proposal_id_from_indexer()
            .await?;
        if id == 0 {
            Ok(1)
        } else {
            Err(anyhow::anyhow!(
                "Fallback to event indexer failed: proposal ID is nonzero"
            ))
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
        // Get bond instructions for the proposal
        let bond_instructions = self.get_bond_instructions(proposal_id).await?;

        let anchor_block_id = anchor_block_info.id();
        // Create new batch
        self.batch_builder
            .create_new_batch(proposal_id, anchor_block_info, bond_instructions);

        Ok(anchor_block_id)
    }

    fn remove_last_l2_block(&mut self) {
        self.batch_builder.remove_last_l2_block();
    }

    pub async fn reset_builder(&mut self) -> Result<(), Error> {
        warn!("Resetting batch builder");
        // TODO handle forced inclusion
        //self.forced_inclusion.sync_queue_index_with_head().await?;

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

    // TODO handle forced inclusion properly
    pub fn has_current_forced_inclusion(&self) -> bool {
        false
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

        let coinbase = block.header.beneficiary();

        let anchor_tx_data = Taiko::get_anchor_tx_data(anchor_tx.input())?;

        let anchor_info = AnchorBlockInfo::from_precomputed_data(
            self.ethereum_l1.execution_layer.common(),
            anchor_tx_data._blockParams.anchorBlockNumber.to::<u64>(),
            anchor_tx_data._blockParams.anchorBlockHash,
            anchor_tx_data._blockParams.anchorStateRoot,
        )
        .await?;

        // TODO imporvee output
        debug!(
            "Recovering from L2 block {}, transactions {}",
            block_height,
            txs.len()
        );

        let txs = txs.to_vec();
        // TODO handle forced inclusion properly

        // TODO validate block params
        self.batch_builder
            .recover_from(
                anchor_tx_data._proposalParams.proposalId.to::<u64>(),
                anchor_info,
                coinbase,
                BondInstructionData::new(
                    anchor_tx_data._proposalParams.bondInstructions,
                    anchor_tx_data._proposalParams.bondInstructionsHash,
                ),
                txs,
                block.header.timestamp(),
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
        l2_slot_info: &L2SlotInfo,
        _is_forced_inclusion: bool,
        allow_forced_inclusion: bool,
    ) -> Result<Option<BuildPreconfBlockResponse>, Error> {
        let l2_block = L2Block::new_from(pending_tx_list, l2_slot_info.slot_timestamp());

        // TODO handle forced inclusion properly

        let block = self
            .add_new_l2_block(
                l2_block,
                l2_slot_info,
                false,
                OperationType::Reanchor,
                allow_forced_inclusion,
            )
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
