mod async_submitter;
mod batch_builder;
pub mod bridge_handler;
pub mod l2_block_payload;
pub mod proposal;

use crate::l1::bindings::{
    FlashLoanReturnMessage, ICheckpointStore::Checkpoint, executeCall,
};
use crate::l1::execution_layer::L1BridgeHandlerOps;
use crate::l2::execution_layer::L2BridgeHandlerOps;
use crate::node::proposal_manager::bridge_handler::UserOp;
use crate::raiko::RaikoClient;
use crate::shared_abi::bindings::IBridge::Message as BridgeMessage;
use crate::{l1::execution_layer::ExecutionLayer, l2::taiko::Taiko};
use alloy::primitives::aliases::U48;
use alloy::primitives::{B256, FixedBytes};
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
        anchor_block_info::AnchorBlockInfo, l2_block_v2::L2BlockV2Draft,
        l2_tx_lists::PreBuiltTxList,
    },
    utils::cancellation_token::CancellationToken,
};
use std::sync::atomic::{AtomicU64, Ordering};
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
    #[allow(dead_code)]
    metrics: Arc<Metrics>,
    #[allow(dead_code)]
    cancel_token: CancellationToken,
    last_finalized_block_hash: B256,
    last_finalized_block_number: Arc<AtomicU64>,
    /// L1→L2 return signal slot discovered during Pass 2 (L2Direct pre-sim).
    /// Pushed into the L2 block's anchor fast signals before real execution
    /// so that `bridge.processMessage(returnMsg, "")` in the UserOp succeeds.
    /// Cleared after each block build.
    pending_return_signal: Option<FixedBytes<32>>,
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
        bridge_rpc_addr: String,
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

        let bridge_addr: SocketAddr = bridge_rpc_addr.parse().map_err(|e| {
            anyhow::anyhow!(
                "Failed to parse BRIDGE_RPC_ADDR '{}': {}",
                bridge_rpc_addr,
                e
            )
        })?;

        let last_finalized_block_number = Arc::new(AtomicU64::new(0));

        let bridge_handler = Arc::new(Mutex::new(
            BridgeHandler::new(
                bridge_addr,
                ethereum_l1.clone(),
                taiko.clone(),
                cancel_token.clone(),
                l1_chain_id,
                l2_chain_id,
                last_finalized_block_number.clone(),
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
            last_finalized_block_number,
            pending_return_signal: None,
        })
    }

    /// Non-blocking poll: check if the in-flight submission has completed.
    /// On success, updates `last_finalized_block_hash`. Returns None if idle or still in progress.
    pub fn poll_submission_result(&mut self) -> Option<Result<(), Error>> {
        match self.async_submitter.try_recv_result() {
            Some(Ok(result)) => {
                info!(
                    "Submission completed. New last finalized block: number={}, hash={}",
                    result.new_last_finalized_block_number, result.new_last_finalized_block_hash,
                );
                self.last_finalized_block_hash = result.new_last_finalized_block_hash;
                self.last_finalized_block_number
                    .store(result.new_last_finalized_block_number, Ordering::Relaxed);
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

        self.batch_builder
            .finalize_if_needed(submit_only_full_batches);

        let Some(batch) = self
            .batch_builder
            .pop_oldest_batch(self.last_finalized_block_hash)
        else {
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
        while let Some(batch) = self
            .batch_builder
            .pop_oldest_batch(self.last_finalized_block_hash)
        {
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

                // --- Pass 2: pre-simulate L2→L1 outbound + L1 callback return ---
                //
                // Dry-run the UserOp to extract any `bridge.sendMessage` it emits
                // (even if the tx reverts on a later `bridge.processMessage`
                // fast-signal check). If found, simulate the L1 callback to
                // discover the L1→L2 return signal. If present, the UserOp's
                // placeholder `returnMessage` is patched with the simulated one.
                let patched_user_op = match self
                    .prepare_l2_direct_user_op(user_op.clone())
                    .await
                {
                    Ok((patched, Some((_return_msg, slot)))) => {
                        info!(
                            "L2Direct pre-sim extracted L1→L2 return slot={} for UserOp id={}",
                            slot, user_op.id
                        );
                        self.pending_return_signal = Some(slot);
                        patched
                    }
                    Ok((patched, None)) => {
                        debug!(
                            "L2Direct pre-sim found no return signal for UserOp id={}",
                            user_op.id
                        );
                        patched
                    }
                    Err(e) => {
                        warn!(
                            "L2Direct pre-sim failed for UserOp id={}: {}. Proceeding with original calldata.",
                            user_op.id, e
                        );
                        user_op.clone()
                    }
                };

                match self
                    .taiko
                    .l2_execution_layer()
                    .construct_l2_user_op_tx(&patched_user_op)
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
                // No L1 UserOp or signal slot returned here for L2-direct ops;
                // `pending_return_signal` on `self` carries any return slot.
                Ok(None)
            }
        }
    }

    /// Pre-simulate an L2Direct UserOp to detect its L2→L1 outbound and the
    /// L1→L2 return produced by the L1 callback. If a return is found and the
    /// UserOp targets `FlashLoanExecutorL2.execute(uint256,address,IBridge.Message)`,
    /// the placeholder returnMessage in its calldata is substituted with the
    /// simulated Message before the real L2 block execution.
    async fn prepare_l2_direct_user_op(
        &self,
        user_op: UserOp,
    ) -> Result<(UserOp, Option<(BridgeMessage, FixedBytes<32>)>), Error> {
        use alloy::primitives::Bytes;

        let l2_el = self.taiko.l2_execution_layer();
        let Some(outbound) = l2_el
            .trace_user_op_for_outbound_message(&user_op)
            .await?
        else {
            return Ok((user_op, None));
        };

        let l1_el = &self.ethereum_l1.execution_layer;
        let bridge_addr = l1_el.contract_addresses().bridge;
        let l2_bridge_addr = *l2_el.bridge.address();
        let Some((return_msg, return_slot)) = l1_el
            .simulate_l1_callback_return_signal(
                outbound,
                Bytes::new(),
                bridge_addr,
                l2_bridge_addr,
            )
            .await?
        else {
            return Ok((user_op, None));
        };

        // Attempt calldata patching.
        let patched_user_op = match maybe_patch_flash_loan_execute_calldata(&user_op, &return_msg) {
            Ok(patched) => patched,
            Err(e) => {
                warn!(
                    "FlashLoanExecutor calldata patch failed ({e}); using original calldata. \
                     The L2 block may revert at processMessage if the placeholder return \
                     Message's hash doesn't match the simulated one."
                );
                user_op.clone()
            }
        };

        Ok((patched_user_op, Some((return_msg, return_slot))))
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
            debug!("No L1→L2 UserOps (L2 direct ops, if any, were handled inline)");
        }

        // If Pass 2 (L2Direct pre-sim) discovered an L1→L2 return signal,
        // inject it into the anchor's fast signals so `bridge.processMessage`
        // inside the UserOp's onFlashLoan / equivalent callback succeeds when
        // the real L2 block executes. The same slot is also recorded in
        // batch_builder.signal_slots so `ProposeInputV2` can split it out as
        // `requiredReturnSignals` when the multicall is built.
        if let Some(return_slot) = self.pending_return_signal.take() {
            info!(
                "Injecting pre-simulated L1→L2 return signal into anchor fast signals: slot={}",
                return_slot
            );
            self.batch_builder.add_signal_slot(return_slot)?;
            anchor_signal_slots.push(return_slot);
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

        // Always update the shared finalized block number for RPC status queries
        self.last_finalized_block_number
            .store(last_proposed_block_number, Ordering::Relaxed);

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

    #[allow(dead_code)]
    pub async fn reanchor_block(
        &mut self,
        pending_tx_list: PreBuiltTxList,
        l2_slot_info: L2SlotInfoV2,
    ) -> Result<BuildPreconfBlockResponse, Error> {
        let l2_slot_context = L2SlotContext {
            info: l2_slot_info,
            end_of_sequencing: false,
        };

        let block = self
            .add_new_l2_block(pending_tx_list, &l2_slot_context, OperationType::Reanchor)
            .await?;

        Ok(block)
    }
}

/// If `user_op.calldata` matches the FlashLoanExecutorL2.execute ABI
/// `execute(uint256 amount, address beneficiary, IBridge.Message returnMessage)`,
/// decode it, substitute `returnMessage` with the simulated `return_msg`, and
/// return a new UserOp with the patched calldata. Otherwise return an error
/// (the caller falls back to the original calldata with a warning).
fn maybe_patch_flash_loan_execute_calldata(
    user_op: &UserOp,
    return_msg: &BridgeMessage,
) -> Result<UserOp, Error> {
    use alloy::primitives::Bytes;
    use alloy::sol_types::SolCall;

    let calldata_bytes = user_op.calldata.as_ref();
    if calldata_bytes.len() < 4 {
        return Err(anyhow::anyhow!("calldata too short for selector"));
    }

    let selector: [u8; 4] = calldata_bytes[0..4]
        .try_into()
        .map_err(|_| anyhow::anyhow!("selector parse failed"))?;

    if selector != executeCall::SELECTOR {
        return Err(anyhow::anyhow!(
            "selector 0x{} does not match FlashLoanExecutor.execute",
            hex_encode(&selector)
        ));
    }

    let decoded = executeCall::abi_decode_raw(&calldata_bytes[4..]).map_err(|e| {
        anyhow::anyhow!("failed to decode execute(uint256,address,(...)) calldata: {e}")
    })?;

    // Build the patched FlashLoanReturnMessage from the simulated BridgeMessage.
    let patched_return = FlashLoanReturnMessage {
        id: return_msg.id,
        fee: return_msg.fee,
        gasLimit: return_msg.gasLimit,
        from: return_msg.from,
        srcChainId: return_msg.srcChainId,
        srcOwner: return_msg.srcOwner,
        destChainId: return_msg.destChainId,
        destOwner: return_msg.destOwner,
        to: return_msg.to,
        value: return_msg.value,
        data: return_msg.data.clone(),
    };

    let patched_call = executeCall {
        amount: decoded.amount,
        beneficiary: decoded.beneficiary,
        returnMessage: patched_return,
    };

    let patched_calldata = Bytes::from(patched_call.abi_encode());
    let mut patched_user_op = user_op.clone();
    patched_user_op.calldata = patched_calldata;
    Ok(patched_user_op)
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}
