mod batch_builder;

use crate::l1::event_indexer::EventIndexer;
use crate::{
    l1::execution_layer::ExecutionLayer,
    l2::taiko::Taiko,
    metrics::Metrics,
    shared::{l2_block::L2Block, l2_slot_info::L2SlotInfo, l2_tx_lists::PreBuiltTxList},
};
use anyhow::Error;
use batch_builder::BatchBuilder;
use common::{
    l1::{ethereum_l1::EthereumL1, traits::ELTrait},
    l2::taiko_driver::{OperationType, models::BuildPreconfBlockResponse},
    shared::anchor_block_info::AnchorBlockInfo,
};
use pacaya::node::batch_manager::config::BatchBuilderConfig;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

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
        event_indexer: Arc<EventIndexer>,
    ) -> Result<(), Error> {
        self.batch_builder
            .try_submit_oldest_batch(
                self.ethereum_l1.clone(),
                submit_only_full_batches,
                event_indexer,
            )
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

    async fn create_new_batch(&mut self) -> Result<u64, Error> {
        // Calculate the anchor block ID and create a new batch
        let anchor_block_info = AnchorBlockInfo::new(
            &self.ethereum_l1.execution_layer.common(),
            self.l1_height_lag,
        ).await?;

        let proposal_id =
            if let Some(current_proposal_id) = self.batch_builder.get_current_proposal_id() {
                current_proposal_id + 1
            } else {
                // TODO get from L2 anchor tx
                1
            };

        // TODO get bond_instructions_hash from event indexer
        let bond_instructions_hash = Default::default();

        let anchor_block_id = anchor_block_info.id();
        // Create new batch
        self.batch_builder.create_new_batch(
            proposal_id,
            anchor_block_info,
            bond_instructions_hash,
        );

        Ok(anchor_block_id)
    }

    async fn calculate_anchor_block_id(&self) -> Result<u64, Error> {
        // TODO get anchor from l2
        let l1_height = self
            .ethereum_l1
            .execution_layer
            .common()
            .get_latest_block_id()
            .await?;
        let l1_height_with_lag = l1_height - self.l1_height_lag;

        Ok(l1_height_with_lag)
    }

    fn remove_last_l2_block(&mut self) {
        self.batch_builder.remove_last_l2_block();
    }
}
