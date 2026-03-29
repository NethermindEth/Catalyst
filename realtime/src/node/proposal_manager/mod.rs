mod async_submitter;
mod batch_builder;
pub mod bridge_handler;
pub mod l2_block_payload;
pub mod proposal;

use crate::l1::bindings::ICheckpointStore::Checkpoint;
use crate::l2::execution_layer::L2BridgeHandlerOps;
use crate::node::proposal_manager::bridge_handler::UserOp;
use crate::raiko::RaikoClient;
use crate::{
    l1::execution_layer::ExecutionLayer,
    l2::taiko::Taiko,
};
use alloy::primitives::{B256, FixedBytes};
use alloy::primitives::aliases::U48;
use anyhow::Error;
use async_submitter::AsyncSubmitter;
use batch_builder::BatchBuilder;
use bridge_handler::BridgeHandler;
use common::metrics::Metrics;
use common::{batch_builder::BatchBuilderConfig, shared::l2_slot_info_v2::L2SlotContext};
use common::{
    l1::{ethereum_l1::EthereumL1, traits::ELTrait},
    l2::taiko_driver::{OperationType, models::BuildPreconfBlockResponse},
    shared::{
        anchor_block_info::AnchorBlockInfo,
        l2_block_v2::{L2BlockV2, L2BlockV2Draft},
        l2_tx_lists::{self, PreBuiltTxList},
    },
    utils::cancellation_token::CancellationToken,
};
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use crate::node::L2SlotInfoV2;

const MIN_ANCHOR_OFFSET: u64 = 2;

pub struct BatchManager {
    batch_builder: BatchBuilder,
    async_submitter: AsyncSubmitter,
    bridge_handler: Arc<Mutex<BridgeHandler>>,
    ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
    pub taiko: Arc<Taiko>,
    l1_height_lag: u64,
    metrics: Arc<Metrics>,
    cancel_token: CancellationToken,
    last_finalized_block_hash: B256,
}

impl BatchManager {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        l1_height_lag: u64,
        config: BatchBuilderConfig,
        ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
        taiko: Arc<Taiko>,
        metrics: Arc<Metrics>,
        cancel_token: CancellationToken,
        last_finalized_block_hash: B256,
        raiko_client: RaikoClient,
        basefee_sharing_pctg: u8,
        proof_request_bypass: bool,
        l1_chain_id: u64,
        l2_chain_id: u64,
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

        let bridge_addr: SocketAddr = "0.0.0.0:4545".parse()?;
        let bridge_handler = Arc::new(Mutex::new(
            BridgeHandler::new(
                bridge_addr,
                ethereum_l1.clone(),
                taiko.clone(),
                cancel_token.clone(),
                l1_chain_id,
                l2_chain_id,
            )
            .await?,
        ));

        let async_submitter = AsyncSubmitter::new(
            raiko_client,
            basefee_sharing_pctg,
            ethereum_l1.clone(),
            proof_request_bypass,
        );

        Ok(Self {
            batch_builder: BatchBuilder::new(
                config,
                ethereum_l1.slot_clock.clone(),
                metrics.clone(),
            ),
            async_submitter,
            bridge_handler,
            ethereum_l1,
            taiko,
            l1_height_lag,
            metrics,
            cancel_token,
            last_finalized_block_hash,
        })
    }

    /// Non-blocking poll: check if the in-flight submission has completed.
    /// On success, updates `last_finalized_block_hash`. Returns None if idle or still in progress.
    pub fn poll_submission_result(&mut self) -> Option<Result<(), Error>> {
        match self.async_submitter.try_recv_result() {
            Some(Ok(result)) => {
                info!(
                    "Submission completed. New last finalized block hash: {}",
                    result.new_last_finalized_block_hash
                );
                self.last_finalized_block_hash = result.new_last_finalized_block_hash;
                Some(Ok(()))
            }
            Some(Err(e)) => Some(Err(e)),
            None => None,
        }
    }

    /// Kick off an async submission if there's a finalized batch ready and the submitter is idle.
    pub async fn try_start_submission(
        &mut self,
        submit_only_full_batches: bool,
    ) -> Result<(), Error> {
        if self.async_submitter.is_busy() {
            return Ok(());
        }

        self.batch_builder.finalize_if_needed(submit_only_full_batches);

        let Some(batch) = self.batch_builder.pop_oldest_batch(self.last_finalized_block_hash) else {
            return Ok(());
        };

        // Check no L1 tx already in progress
        if self
            .ethereum_l1
            .execution_layer
            .is_transaction_in_progress()
            .await?
        {
            debug!("Cannot submit batch, L1 transaction already in progress. Re-queuing.");
            self.batch_builder.push_front_batch(batch);
            return Ok(());
        }

        let status_store = self.bridge_handler.lock().await.status_store();

        info!(
            "Starting async submission: {} blocks, last_finalized_block_hash: {}",
            batch.l2_blocks.len(),
            batch.last_finalized_block_hash,
        );

        self.async_submitter.submit(batch, Some(status_store));
        Ok(())
    }

    pub fn is_submission_in_progress(&self) -> bool {
        self.async_submitter.is_busy()
    }

    /// Drop all finalized batches without submitting. Used in PRECONF_ONLY mode.
    pub fn drain_finalized_batches(&mut self) {
        self.batch_builder.finalize_if_needed(false);
        while let Some(batch) = self.batch_builder.pop_oldest_batch(self.last_finalized_block_hash) {
            info!(
                "PRECONF_ONLY: dropping batch with {} blocks",
                batch.l2_blocks.len(),
            );
        }
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
            info!("Maximum allowed anchor height offset exceeded, finalizing current batch.");
            self.batch_builder.finalize_current_batch();
        }

        Ok(result)
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
            "Adding new L2 block id: {}, timestamp: {}",
            l2_slot_context.info.parent_id() + 1,
            timestamp,
        );

        let l2_draft_block = L2BlockV2Draft {
            prebuilt_tx_list: prebuilt_tx_list.clone(),
            timestamp_sec: timestamp,
            gas_limit_without_anchor: l2_slot_context.info.parent_gas_limit_without_anchor(),
        };

        if !self.batch_builder.can_consume_l2_block(&l2_draft_block) {
            let _ = self.create_new_batch().await?;
        }

        let preconfed_block = self
            .add_draft_block_to_proposal(l2_draft_block, l2_slot_context, operation_type)
            .await?;

        Ok(preconfed_block)
    }

    pub async fn has_pending_user_ops(&self) -> bool {
        self.bridge_handler.lock().await.has_pending_user_ops()
    }

    /// Process all pending UserOps: route each to L1 or L2 based on its chainId field.
    ///
    /// - L1→L2 deposits: UserOp added to proposal (for L1 multicall), processMessage tx added to L2 block
    /// - L2 direct (bridge-out): UserOp execution tx added to L2 block, L2→L1 relay handled post-execution
    async fn add_pending_user_ops_to_draft_block(
        &mut self,
        l2_draft_block: &mut L2BlockV2Draft,
    ) -> Result<Option<(UserOp, FixedBytes<32>)>, anyhow::Error> {
        use bridge_handler::UserOpRouting;

        let (routing, status_store) = {
            let mut handler = self.bridge_handler.lock().await;
            let routing = handler.next_user_op_routed().await?;
            (routing, handler.status_store())
        };

        let Some(routing) = routing else {
            return Ok(None);
        };

        match routing {
            UserOpRouting::L1ToL2 { user_op, l2_call } => {
                info!("Processing L1→L2 deposit: UserOp id={}", user_op.id);

                let l2_call_bridge_tx = self
                    .taiko
                    .l2_execution_layer()
                    .construct_l2_call_tx(l2_call.message_from_l1)
                    .await?;

                info!("Inserting processMessage tx into L2 block");
                l2_draft_block
                    .prebuilt_tx_list
                    .tx_list
                    .push(l2_call_bridge_tx);

                Ok(Some((user_op, l2_call.signal_slot_on_l2)))
            }
            UserOpRouting::L2Direct { user_op } => {
                info!(
                    "Processing L2 UserOp (bridge-out): id={} submitter={}",
                    user_op.id, user_op.submitter
                );

                match self
                    .taiko
                    .l2_execution_layer()
                    .construct_l2_user_op_tx(&user_op)
                    .await
                {
                    Ok(tx) => {
                        // Track L2 UserOp ID first — only insert tx if tracking succeeds,
                        // otherwise we'd execute on L2 but show Rejected in the status store.
                        if let Err(e) = self.batch_builder.add_l2_user_op_id(user_op.id) {
                            error!(
                                "Failed to track L2 UserOp id={}: {}. Dropping tx.",
                                user_op.id, e
                            );
                            status_store.set(
                                user_op.id,
                                &bridge_handler::UserOpStatus::Rejected {
                                    reason: format!("Failed to track UserOp: {}", e),
                                },
                            );
                        } else {
                            info!("Inserting L2 UserOp execution tx into block");
                            l2_draft_block.prebuilt_tx_list.tx_list.push(tx);
                        }
                    }
                    Err(e) => {
                        error!("Failed to construct L2 UserOp tx: {}", e);
                        status_store.set(
                            user_op.id,
                            &bridge_handler::UserOpStatus::Rejected {
                                reason: format!("Failed to construct L2 tx: {}", e),
                            },
                        );
                    }
                }
                // No L1 UserOp or signal slot for L2-direct ops
                Ok(None)
            }
        }
    }

    async fn add_draft_block_to_proposal(
        &mut self,
        mut l2_draft_block: L2BlockV2Draft,
        l2_slot_context: &L2SlotContext,
        operation_type: OperationType,
    ) -> Result<BuildPreconfBlockResponse, Error> {
        let mut anchor_signal_slots: Vec<FixedBytes<32>> = vec![];

        debug!("Checking for pending UserOps (L1→L2 deposits and L2 direct)");
        if let Some((user_op_data, signal_slot)) = self
            .add_pending_user_ops_to_draft_block(&mut l2_draft_block)
            .await?
        {
            self.batch_builder.add_user_op(user_op_data)?;
            self.batch_builder.add_signal_slot(signal_slot)?;
            anchor_signal_slots.push(signal_slot);
        } else {
            debug!("No pending UserOps");
        }

        let payload = self.batch_builder.add_l2_draft_block(l2_draft_block)?;

        match self
            .taiko
            .advance_head_to_new_l2_block(
                payload,
                l2_slot_context,
                anchor_signal_slots,
                operation_type,
            )
            .await
        {
            Ok(preconfed_block) => {
                self.batch_builder.set_proposal_checkpoint(Checkpoint {
                    blockNumber: U48::from(preconfed_block.number),
                    stateRoot: preconfed_block.state_root,
                    blockHash: preconfed_block.hash,
                })?;

                debug!("Checking for initiated L1 calls");
                if let Some(l1_call) = self
                    .bridge_handler
                    .lock()
                    .await
                    .find_l1_call(preconfed_block.number, preconfed_block.state_root)
                    .await?
                {
                    self.batch_builder.add_l1_call(l1_call)?;
                } else {
                    debug!("No L1 calls initiated");
                }

                Ok(preconfed_block)
            }
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

        let anchor_block_id = anchor_block_info.id();
        // Use B256::ZERO as placeholder -- real last_finalized_block_hash is stamped at submission time
        self.batch_builder
            .create_new_batch(anchor_block_info, B256::ZERO);

        Ok(anchor_block_id)
    }

    fn remove_last_l2_block(&mut self) {
        self.batch_builder.remove_last_l2_block();
    }

    pub async fn reset_builder(&mut self) -> Result<(), Error> {
        warn!("Resetting batch builder");

        self.async_submitter.abort();

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

    pub fn get_number_of_batches(&self) -> u64 {
        self.batch_builder.get_number_of_batches()
    }

    /// Reorg all unproposed L2 blocks back to the last proposed block.
    /// Called on startup to clean up any preconfirmed-but-unproposed blocks.
    pub async fn reorg_unproposed_blocks(&mut self) -> Result<(), Error> {
        let last_finalized_hash = self
            .ethereum_l1
            .execution_layer
            .get_last_finalized_block_hash()
            .await?;

        if last_finalized_hash == B256::ZERO {
            info!("No finalized block hash on L1 (genesis). Nothing to reorg.");
            return Ok(());
        }

        let last_proposed_block_number = match self
            .taiko
            .find_l2_block_number_by_hash(last_finalized_hash)
            .await
        {
            Ok(n) => n,
            Err(_) => {
                info!(
                    "lastFinalizedBlockHash {} not found on L2 — treating as no blocks proposed yet",
                    last_finalized_hash
                );
                0
            }
        };

        let l2_head = self.taiko.get_latest_l2_block_id().await?;

        if l2_head <= last_proposed_block_number {
            info!(
                "No unproposed blocks: L2 head {} <= last proposed {}",
                l2_head, last_proposed_block_number
            );
            return Ok(());
        }

        let gap = l2_head - last_proposed_block_number;
        warn!(
            "Detected {} unproposed L2 blocks ({} to {}). Reorging to last proposed block {}.",
            gap,
            last_proposed_block_number + 1,
            l2_head,
            last_proposed_block_number
        );

        let reorg_result = self
            .taiko
            .reorg_stale_block(last_proposed_block_number)
            .await?;
        info!(
            "Reorg complete: new head hash={}, blocks removed={}",
            reorg_result.new_head_block_hash, reorg_result.blocks_removed
        );

        self.last_finalized_block_hash = last_finalized_hash;
        Ok(())
    }


    pub async fn reanchor_block(
        &mut self,
        pending_tx_list: PreBuiltTxList,
        l2_slot_info: L2SlotInfoV2,
    ) -> Result<BuildPreconfBlockResponse, Error> {
        let l2_slot_context = L2SlotContext {
            info: l2_slot_info,
            end_of_sequencing: false,
            allow_forced_inclusion: false,
        };

        let block = self
            .add_new_l2_block(pending_tx_list, &l2_slot_context, OperationType::Reanchor)
            .await?;

        Ok(block)
    }
}
