pub mod proposal_manager;
use anyhow::Error;
use common::{
    fork_info::ForkInfo,
    l1::{ethereum_l1::EthereumL1, transaction_error::TransactionError},
    l2::taiko_driver::{TaikoDriver, models::BuildPreconfBlockResponse},
    metrics::Metrics,
    shared::{l2_slot_info_v2::L2SlotContext, l2_tx_lists::PreBuiltTxList},
    utils::{self as common_utils, cancellation_token::CancellationToken},
};
use pacaya::node::operator::Status as OperatorStatus;
use pacaya::node::{config::NodeConfig, operator::Operator};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::l1::execution_layer::ExecutionLayer;
use crate::l2::taiko::Taiko;
use common::batch_builder::BatchBuilderConfig;
use common::l1::traits::PreconferProvider;
use common::shared::head_verifier::HeadVerifier;
use common::shared::l2_slot_info_v2::L2SlotInfoV2;
use proposal_manager::BatchManager;

use tokio::{
    sync::mpsc::{Receiver, error::TryRecvError},
    time::{Duration, sleep},
};

pub struct Node {
    config: NodeConfig,
    cancel_token: CancellationToken,
    ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
    taiko: Arc<Taiko>,
    watchdog: common_utils::watchdog::Watchdog,
    operator: Operator<ExecutionLayer, common::l1::slot_clock::RealClock, TaikoDriver>,
    metrics: Arc<Metrics>,
    proposal_manager: BatchManager,
    head_verifier: HeadVerifier,
    transaction_error_channel: Receiver<TransactionError>,
    preconf_only: bool,
}

impl Node {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        config: NodeConfig,
        cancel_token: CancellationToken,
        ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
        taiko: Arc<Taiko>,
        metrics: Arc<Metrics>,
        batch_builder_config: BatchBuilderConfig,
        transaction_error_channel: Receiver<TransactionError>,
        fork_info: ForkInfo,
        last_finalized_block_hash: alloy::primitives::B256,
        raiko_client: crate::raiko::RaikoClient,
        basefee_sharing_pctg: u8,
        preconf_only: bool,
        proof_request_bypass: bool,
        l1_chain_id: u64,
        l2_chain_id: u64,
    ) -> Result<Self, Error> {
        let operator = Operator::new(
            ethereum_l1.execution_layer.clone(),
            ethereum_l1.slot_clock.clone(),
            taiko.get_driver(),
            config.handover_window_slots,
            config.handover_start_buffer_ms,
            config.simulate_not_submitting_at_the_end_of_epoch,
            cancel_token.clone(),
            fork_info.clone(),
        )
        .map_err(|e| anyhow::anyhow!("Failed to create Operator: {}", e))?;
        let watchdog = common_utils::watchdog::Watchdog::new(
            cancel_token.clone(),
            ethereum_l1.slot_clock.get_l2_slots_per_epoch() / 2,
        );
        let head_verifier = HeadVerifier::default();

        let proposal_manager = BatchManager::new(
            config.l1_height_lag,
            batch_builder_config,
            ethereum_l1.clone(),
            taiko.clone(),
            metrics.clone(),
            cancel_token.clone(),
            last_finalized_block_hash,
            raiko_client,
            basefee_sharing_pctg,
            proof_request_bypass,
            l1_chain_id,
            l2_chain_id,
        )
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create BatchManager: {}", e))?;

        let start = std::time::Instant::now();
        common::blob::build_default_kzg_settings();
        info!(
            "Setup build_default_kzg_settings in {} milliseconds",
            start.elapsed().as_millis()
        );

        Ok(Self {
            config,
            cancel_token,
            ethereum_l1,
            taiko,
            watchdog,
            operator,
            metrics,
            proposal_manager,
            head_verifier,
            transaction_error_channel,
            preconf_only,
        })
    }

    pub async fn entrypoint(mut self) -> Result<(), Error> {
        info!("Starting RealTime node");

        if let Err(err) = self.warmup().await {
            error!("Failed to warm up node: {}. Shutting down.", err);
            self.cancel_token.cancel_on_critical_error();
            return Err(anyhow::anyhow!(err));
        }

        info!("Node warmup successful");

        tokio::spawn(async move {
            self.preconfirmation_loop().await;
        });

        Ok(())
    }

    async fn preconfirmation_loop(&mut self) {
        debug!("Main preconfirmation loop started");
        common_utils::synchronization::synchronize_with_l1_slot_start(&self.ethereum_l1).await;

        let mut interval =
            tokio::time::interval(Duration::from_millis(self.config.preconf_heartbeat_ms));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;

            if self.cancel_token.is_cancelled() {
                info!("Shutdown signal received, exiting main loop...");
                return;
            }

            if let Err(err) = self.main_block_preconfirmation_step().await {
                error!("Failed to execute main block preconfirmation step: {}", err);
                self.watchdog.increment();
            } else {
                self.watchdog.reset();
            }
        }
    }

    async fn main_block_preconfirmation_step(&mut self) -> Result<(), Error> {
        let (l2_slot_info, current_status, pending_tx_list) =
            self.get_slot_info_and_status().await?;

        if !self.preconf_only {
            // Poll for completed async submissions (non-blocking)
            if let Some(result) = self.proposal_manager.poll_submission_result() {
                match result {
                    Ok(()) => info!("Async submission completed successfully"),
                    Err(e) => {
                        if let Some(transaction_error) = e.downcast_ref::<TransactionError>() {
                            self.handle_transaction_error(
                                transaction_error,
                                &current_status,
                                &l2_slot_info,
                            )
                            .await?;
                        } else {
                            error!("Async submission failed: {}. Restarting node.", e);
                            self.cancel_token.cancel_on_critical_error();
                            return Err(anyhow::anyhow!("Async submission failed: {}", e));
                        }
                    }
                }
            }

            self.check_transaction_error_channel(&current_status, &l2_slot_info)
                .await?;
        }

        if current_status.is_preconfirmation_start_slot() {
            self.head_verifier
                .set(l2_slot_info.parent_id(), *l2_slot_info.parent_hash())
                .await;
        }

        // Preconfirmation phase — skip if a proof request or submission is already in progress
        if current_status.is_preconfer()
            && current_status.is_driver_synced()
            && !self.proposal_manager.is_submission_in_progress()
        {
            if !self
                .head_verifier
                .verify(l2_slot_info.parent_id(), l2_slot_info.parent_hash())
                .await
            {
                self.head_verifier.log_error().await;
                self.cancel_token.cancel_on_critical_error();
                return Err(anyhow::anyhow!(
                    "Unexpected L2 head detected. Restarting node..."
                ));
            }

            let l2_slot_context = L2SlotContext {
                info: l2_slot_info.clone(),
                end_of_sequencing: current_status.is_end_of_sequencing(),
                allow_forced_inclusion: false,
            };

            if self
                .proposal_manager
                .should_new_block_be_created(&pending_tx_list, &l2_slot_context)
                && (pending_tx_list
                    .as_ref()
                    .is_some_and(|pre_built_list| !pre_built_list.tx_list.is_empty())
                    || self.proposal_manager.has_pending_user_ops().await)
            {
                let preconfed_block = self
                    .proposal_manager
                    .preconfirm_block(pending_tx_list, &l2_slot_context)
                    .await?;

                self.verify_preconfed_block(preconfed_block).await?;
            }
        }

        // Submission phase
        if self.preconf_only {
            // PRECONF_ONLY mode: drop finalized batches without proving/proposing
            self.proposal_manager.drain_finalized_batches();
        } else if current_status.is_submitter()
            && !self.proposal_manager.is_submission_in_progress()
            && let Err(err) = self
                .proposal_manager
                .try_start_submission(current_status.is_preconfer())
                .await
        {
            if let Some(transaction_error) = err.downcast_ref::<TransactionError>() {
                self.handle_transaction_error(transaction_error, &current_status, &l2_slot_info)
                    .await?;
            } else {
                return Err(err);
            }
        }

        // Cleanup
        if !current_status.is_submitter()
            && !current_status.is_preconfer()
            && self.proposal_manager.has_batches()
        {
            error!(
                "Resetting batch builder. has batches: {}",
                self.proposal_manager.has_batches(),
            );
            self.proposal_manager.reset_builder().await?;
        }

        Ok(())
    }

    async fn handle_transaction_error(
        &mut self,
        error: &TransactionError,
        _current_status: &OperatorStatus,
        _l2_slot_info: &L2SlotInfoV2,
    ) -> Result<(), Error> {
        match error {
            TransactionError::ReanchorRequired => {
                warn!("Unexpected ReanchorRequired error received");
                self.cancel_token.cancel_on_critical_error();
                Err(anyhow::anyhow!(
                    "ReanchorRequired error received unexpectedly, exiting"
                ))
            }
            TransactionError::NotConfirmed => {
                self.cancel_token.cancel_on_critical_error();
                Err(anyhow::anyhow!(
                    "Transaction not confirmed for a long time, exiting"
                ))
            }
            TransactionError::UnsupportedTransactionType => {
                self.cancel_token.cancel_on_critical_error();
                Err(anyhow::anyhow!("Unsupported transaction type"))
            }
            TransactionError::GetBlockNumberFailed => {
                self.cancel_token.cancel_on_critical_error();
                Err(anyhow::anyhow!("Failed to get block number from L1"))
            }
            TransactionError::EstimationTooEarly => {
                warn!("Transaction estimation too early");
                Ok(())
            }
            TransactionError::InsufficientFunds => {
                self.cancel_token.cancel_on_critical_error();
                Err(anyhow::anyhow!(
                    "Transaction reverted with InsufficientFunds error"
                ))
            }
            TransactionError::EstimationFailed => {
                self.cancel_token.cancel_on_critical_error();
                Err(anyhow::anyhow!("Transaction estimation failed, exiting"))
            }
            TransactionError::TransactionReverted => {
                self.cancel_token.cancel_on_critical_error();
                Err(anyhow::anyhow!("Transaction reverted, exiting"))
            }
            TransactionError::OldestForcedInclusionDue => {
                // No forced inclusions in RealTime, but handle gracefully
                warn!("OldestForcedInclusionDue received in RealTime mode, ignoring");
                Ok(())
            }
            TransactionError::NotTheOperatorInCurrentEpoch => {
                warn!("Propose batch transaction executed too late.");
                Ok(())
            }
        }
    }

    async fn get_slot_info_and_status(
        &mut self,
    ) -> Result<(L2SlotInfoV2, OperatorStatus, Option<PreBuiltTxList>), Error> {
        let l2_slot_info = self.taiko.get_l2_slot_info().await;
        let current_status = match &l2_slot_info {
            Ok(info) => self.operator.get_status(info).await,
            Err(_) => Err(anyhow::anyhow!("Failed to get L2 slot info")),
        };

        let gas_limit_without_anchor = match &l2_slot_info {
            Ok(info) => info.parent_gas_limit_without_anchor(),
            Err(_) => {
                error!("Failed to get L2 slot info set gas_limit_without_anchor to 0");
                0u64
            }
        };

        let pending_tx_list = if gas_limit_without_anchor != 0 {
            let batches_ready_to_send = 0;
            match &l2_slot_info {
                Ok(info) => {
                    self.taiko
                        .get_pending_l2_tx_list_from_l2_engine(
                            info.base_fee(),
                            batches_ready_to_send,
                            gas_limit_without_anchor,
                        )
                        .await
                }
                Err(_) => Err(anyhow::anyhow!("Failed to get L2 slot info")),
            }
        } else {
            Ok(None)
        };

        self.print_current_slots_info(
            &current_status,
            &pending_tx_list,
            &l2_slot_info,
            self.proposal_manager.get_number_of_batches(),
        )?;

        Ok((l2_slot_info?, current_status?, pending_tx_list?))
    }

    async fn verify_preconfed_block(
        &self,
        l2_block: BuildPreconfBlockResponse,
    ) -> Result<(), Error> {
        if !self
            .head_verifier
            .verify_next_and_set(l2_block.number, l2_block.hash, l2_block.parent_hash)
            .await
        {
            self.head_verifier.log_error().await;
            self.cancel_token.cancel_on_critical_error();
            return Err(anyhow::anyhow!(
                "Unexpected L2 head after preconfirmation. Restarting node..."
            ));
        }
        Ok(())
    }

    async fn check_transaction_error_channel(
        &mut self,
        current_status: &OperatorStatus,
        l2_slot_info: &L2SlotInfoV2,
    ) -> Result<(), Error> {
        match self.transaction_error_channel.try_recv() {
            Ok(error) => {
                self.handle_transaction_error(&error, current_status, l2_slot_info)
                    .await
            }
            Err(err) => match err {
                TryRecvError::Empty => Ok(()),
                TryRecvError::Disconnected => {
                    self.cancel_token.cancel_on_critical_error();
                    Err(anyhow::anyhow!("Transaction error channel disconnected"))
                }
            },
        }
    }

    fn print_current_slots_info(
        &self,
        current_status: &Result<OperatorStatus, Error>,
        pending_tx_list: &Result<Option<PreBuiltTxList>, Error>,
        l2_slot_info: &Result<L2SlotInfoV2, Error>,
        batches_number: u64,
    ) -> Result<(), Error> {
        let l1_slot = self.ethereum_l1.slot_clock.get_current_slot()?;
        info!(target: "heartbeat",
            "| Epoch: {:<6} | Slot: {:<2} | L2 Slot: {:<2} | {}{} Batches: {batches_number} | {} |",
            self.ethereum_l1.slot_clock.get_epoch_from_slot(l1_slot),
            self.ethereum_l1.slot_clock.slot_of_epoch(l1_slot),
            self.ethereum_l1
                .slot_clock
                .get_current_l2_slot_within_l1_slot()?,
            if let Ok(pending_tx_list) = pending_tx_list {
                format!(
                    "Txs: {:<4} |",
                    pending_tx_list
                        .as_ref()
                        .map_or(0, |tx_list| tx_list.tx_list.len())
                )
            } else {
                "Txs: unknown |".to_string()
            },
            if let Ok(l2_slot_info) = l2_slot_info {
                format!(
                    " Fee: {:<7} | L2: {:<6} | Time: {:<10} | Hash: {} |",
                    l2_slot_info.base_fee(),
                    l2_slot_info.parent_id(),
                    l2_slot_info.slot_timestamp(),
                    &l2_slot_info.parent_hash().to_string()[..8]
                )
            } else {
                " L2 slot info unknown |".to_string()
            },
            if let Ok(status) = current_status {
                status.to_string()
            } else {
                "Unknown".to_string()
            },
        );
        Ok(())
    }

    async fn warmup(&mut self) -> Result<(), Error> {
        info!("Warmup RealTime node");

        // Wait for RealTimeInbox activation (lastFinalizedBlockHash != 0)
        loop {
            let hash = self
                .ethereum_l1
                .execution_layer
                .get_last_finalized_block_hash()
                .await?;
            if hash != alloy::primitives::B256::ZERO {
                info!("RealTimeInbox is active, lastFinalizedBlockHash: {}", hash);
                break;
            }
            warn!("RealTimeInbox not yet activated. Waiting...");
            sleep(Duration::from_secs(12)).await;
        }

        // Wait for the last sent transaction to be executed
        self.wait_for_sent_transactions().await?;

        // Reorg any preconfirmed-but-unproposed L2 blocks back to the last proposed block
        if !self.preconf_only {
            self.proposal_manager.reorg_unproposed_blocks().await?;
        }

        Ok(())
    }

    async fn wait_for_sent_transactions(&self) -> Result<(), Error> {
        loop {
            let nonce_latest: u64 = self
                .ethereum_l1
                .execution_layer
                .get_preconfer_nonce_latest()
                .await?;
            let nonce_pending: u64 = self
                .ethereum_l1
                .execution_layer
                .get_preconfer_nonce_pending()
                .await?;
            if nonce_pending == nonce_latest {
                break;
            }
            debug!(
                "Waiting for sent transactions to be executed. Nonce Latest: {nonce_latest}, Nonce Pending: {nonce_pending}"
            );
            sleep(Duration::from_secs(6)).await;
        }

        Ok(())
    }
}
