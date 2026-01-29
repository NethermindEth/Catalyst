pub mod batch_manager;
pub mod config;
pub mod operator;
mod verifier;

use crate::{
    chain_monitor::PacayaChainMonitor,
    l1::execution_layer::ExecutionLayer,
    l2::taiko::Taiko,
    metrics::Metrics,
    shared::{head_verifier::HeadVerifier, l2_slot_info::L2SlotInfo, l2_tx_lists::PreBuiltTxList},
};
use anyhow::Error;
use batch_manager::BatchManager;
use common::batch_builder::BatchBuilderConfig;
use common::{
    fork_info::ForkInfo,
    l1::{ethereum_l1::EthereumL1, traits::PreconferProvider, transaction_error::TransactionError},
    l2::taiko_driver::{OperationType, TaikoDriver, models::BuildPreconfBlockResponse},
    utils as common_utils,
    utils::cancellation_token::CancellationToken,
};
use config::NodeConfig;
use operator::{Operator, Status as OperatorStatus};
use std::sync::Arc;
use tokio::{
    sync::mpsc::{Receiver, error::TryRecvError},
    time::{Duration, sleep},
};
use tracing::{debug, error, info, warn};
use verifier::{VerificationResult, Verifier};

pub struct Node {
    cancel_token: CancellationToken,
    ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
    chain_monitor: Arc<PacayaChainMonitor>,
    operator: Operator<ExecutionLayer, common::l1::slot_clock::RealClock, TaikoDriver>,
    batch_manager: BatchManager,
    verifier: Option<Verifier>,
    taiko: Arc<Taiko>,
    transaction_error_channel: Receiver<TransactionError>,
    metrics: Arc<Metrics>,
    watchdog: common_utils::watchdog::Watchdog,
    head_verifier: HeadVerifier,
    config: NodeConfig,
    fork_info: ForkInfo,
}

impl Node {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        cancel_token: CancellationToken,
        taiko: Arc<Taiko>,
        ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
        chain_monitor: Arc<PacayaChainMonitor>,
        transaction_error_channel: Receiver<TransactionError>,
        metrics: Arc<Metrics>,
        config: NodeConfig,
        batch_builder_config: BatchBuilderConfig,
        fork_info: ForkInfo,
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
        let batch_manager = BatchManager::new(
            config.l1_height_lag,
            batch_builder_config,
            ethereum_l1.clone(),
            taiko.clone(),
            metrics.clone(),
            cancel_token.clone(),
        )
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create BatchManager: {}", e))?;
        // Workaround for the issue: https://github.com/NethermindEth/Catalyst/issues/611
        // e2e-test to reproduce issue: test_preocnfirmation_after_restart
        let start = std::time::Instant::now();
        common::blob::build_default_kzg_settings();
        info!(
            "Setup build_default_kzg_settings in {} milliseconds",
            start.elapsed().as_millis()
        );
        let head_verifier = HeadVerifier::default();
        let watchdog = common_utils::watchdog::Watchdog::new(
            cancel_token.clone(),
            ethereum_l1.slot_clock.get_l2_slots_per_epoch() / 2,
        );
        Ok(Self {
            cancel_token,
            batch_manager,
            ethereum_l1,
            chain_monitor,
            operator,
            verifier: None,
            taiko,
            transaction_error_channel,
            metrics,
            watchdog,
            head_verifier,
            config,
            fork_info,
        })
    }

    pub async fn entrypoint(mut self) -> Result<(), Error> {
        info!("Starting node");

        if let Err(err) = self.warmup().await {
            error!("Failed to warm up node: {}. Shutting down.", err);
            self.cancel_token.cancel_on_critical_error();
            return Err(anyhow::anyhow!(err));
        }

        info!("Node warmup successful");

        // Run preconfirmation loop in background
        tokio::spawn(async move {
            self.preconfirmation_loop().await;
        });

        Ok(())
    }

    async fn get_current_protocol_height(&self) -> Result<(u64, u64), Error> {
        let taiko_inbox_height = self
            .ethereum_l1
            .execution_layer
            .get_l2_height_from_taiko_inbox()
            .await?;

        let taiko_geth_height = self.taiko.get_latest_l2_block_id().await?;

        Ok((taiko_inbox_height, taiko_geth_height))
    }

    async fn warmup(&mut self) -> Result<(), Error> {
        info!("Warmup node");

        // Wait for Taiko Geth to synchronize with L1
        let (mut taiko_inbox_height, mut taiko_geth_height) =
            self.get_current_protocol_height().await?;

        info!("Taiko Inbox Height: {taiko_inbox_height}, Taiko Geth Height: {taiko_geth_height}");

        while taiko_geth_height < taiko_inbox_height {
            warn!("Taiko Geth is behind L1. Waiting 5 seconds...");
            sleep(Duration::from_secs(5)).await;

            (taiko_inbox_height, taiko_geth_height) = self.get_current_protocol_height().await?;

            info!(
                "Taiko Inbox Height: {taiko_inbox_height}, Taiko Geth Height: {taiko_geth_height}"
            );
        }

        // Wait for the last sent transaction to be executed
        self.wait_for_sent_transactions().await?;

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

    async fn preconfirmation_loop(&mut self) {
        debug!("Main preconfirmation loop started");
        common_utils::synchronization::synchronize_with_l1_slot_start(&self.ethereum_l1).await;

        let mut interval =
            tokio::time::interval(Duration::from_millis(self.config.preconf_heartbeat_ms));
        // fix for handover buffer longer than l2 heart beat, keeps the loop synced
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

    async fn recreate_node_when_next_fork_became_active(
        &self,
        timestamp_sec: u64,
    ) -> Result<bool, Error> {
        if self.fork_info.is_next_fork_active(timestamp_sec)? {
            debug!("Next fork became active, recreating node...");
            self.cancel_token.cancel();
            return Ok(true);
        }

        Ok(false)
    }

    async fn check_for_missing_proposed_batches(&mut self) -> Result<(), Error> {
        let (taiko_inbox_height, taiko_geth_height) = self.get_current_protocol_height().await?;

        info!(
            "📨 Taiko Inbox Height: {taiko_inbox_height}, Taiko Geth Height: {taiko_geth_height}"
        );

        if taiko_inbox_height == taiko_geth_height {
            return Ok(());
        } else {
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
            debug!("Nonce Latest: {nonce_latest}, Nonce Pending: {nonce_pending}");
            if nonce_latest == nonce_pending {
                // Just create a new verifier, we will check it in preconfirmation loop
                self.verifier = Some(
                    Verifier::new_with_taiko_height(
                        taiko_geth_height,
                        self.taiko.clone(),
                        self.batch_manager
                            .update_forced_inclusion_and_clone_without_batches()
                            .await?,
                        0,
                        self.cancel_token.clone(),
                    )
                    .await?,
                );
            } else {
                error!(
                    "Error: Pending nonce is not equal to latest nonce. Nonce Latest: {nonce_latest}, Nonce Pending: {nonce_pending}"
                );
                return Err(Error::msg("Pending nonce is not equal to latest nonce"));
            }
        }

        Ok(())
    }

    async fn main_block_preconfirmation_step(&mut self) -> Result<(), Error> {
        let (l2_slot_info, current_status, pending_tx_list) =
            self.get_slot_info_and_status().await?;

        if self
            .recreate_node_when_next_fork_became_active(l2_slot_info.slot_timestamp())
            .await
            .map_err(|e| anyhow::anyhow!("Failed to check if next fork became active: {}", e))?
        {
            return Ok(());
        }

        // Get the transaction status before checking the error channel
        // to avoid race condition
        let transaction_in_progress = self
            .ethereum_l1
            .execution_layer
            .is_transaction_in_progress()
            .await?;

        self.check_transaction_error_channel(&current_status, &l2_slot_info)
            .await?;

        if current_status.is_preconfirmation_start_slot() {
            self.head_verifier
                .set(l2_slot_info.parent_id(), *l2_slot_info.parent_hash())
                .await;

            if current_status.is_submitter() {
                // We start preconfirmation in the middle of the epoch.
                // Need to check for unproposed L2 blocks.
                if let Err(err) = self.check_for_missing_proposed_batches().await {
                    error!(
                        "Shutdown: Failed to verify proposed batches on startup: {}",
                        err
                    );
                    self.cancel_token.cancel_on_critical_error();
                    return Err(anyhow::anyhow!(
                        "Shutdown: Failed to verify proposed batches on startup: {}",
                        err
                    ));
                }
            } else {
                // It is for handover window
                let taiko_geth_height = l2_slot_info.parent_id();
                let verification_slot = self.ethereum_l1.slot_clock.get_next_epoch_start_slot()?;
                let verifier_result = Verifier::new_with_taiko_height(
                    taiko_geth_height,
                    self.taiko.clone(),
                    self.batch_manager
                        .update_forced_inclusion_and_clone_without_batches()
                        .await?,
                    verification_slot,
                    self.cancel_token.clone(),
                )
                .await;
                match verifier_result {
                    Ok(verifier) => {
                        self.verifier = Some(verifier);
                    }
                    Err(err) => {
                        error!("Shutdown: Failed to create verifier: {}", err);
                        self.cancel_token.cancel_on_critical_error();
                        return Err(anyhow::anyhow!(
                            "Shutdown: Failed to create verifier on startup: {}",
                            err
                        ));
                    }
                }
            }
        }

        if current_status.is_preconfer() && current_status.is_driver_synced() {
            // do not trigger fast reanchor on submitter window to prevent from double reanchor
            if !current_status.is_submitter()
                && self
                    .check_and_handle_anchor_offset_for_unsafe_l2_blocks(&l2_slot_info)
                    .await?
            {
                // reanchored, no need to preconf
                return Ok(());
            }

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
            let preconfed_block = self
                .preconfirm_block(
                    pending_tx_list,
                    &l2_slot_info,
                    current_status.is_end_of_sequencing(),
                    self.config.propose_forced_inclusion
                        && current_status.is_submitter()
                        && self.verifier.is_none(),
                )
                .await?;

            self.verify_preconfed_block(preconfed_block).await?;
        }

        if current_status.is_submitter() && !transaction_in_progress {
            // first check verifier
            if self.has_verified_unproposed_batches().await?
                && let Err(err) = self
                    .batch_manager
                    .try_submit_oldest_batch(current_status.is_preconfer())
                    .await
            {
                if let Some(transaction_error) = err.downcast_ref::<TransactionError>() {
                    self.handle_transaction_error(
                        transaction_error,
                        &current_status,
                        &l2_slot_info,
                    )
                    .await?;
                } else {
                    return Err(err);
                }
            }
        }

        if !current_status.is_submitter() && !current_status.is_preconfer() {
            if self.batch_manager.has_batches() || self.batch_manager.has_current_forced_inclusion()
            {
                error!(
                    "Resetting batch builder. has batches: {}, has current forced inclusion: {}",
                    self.batch_manager.has_batches(),
                    self.batch_manager.has_current_forced_inclusion()
                );
                self.batch_manager.reset_builder().await?;
            }
            if self.verifier.is_some() {
                error!("Verifier is not None after submitter window.");
                self.verifier = None;
            }
        }

        Ok(())
    }

    async fn verify_preconfed_block(
        &self,
        l2_block: Option<BuildPreconfBlockResponse>,
    ) -> Result<(), Error> {
        if let Some(l2_block) = l2_block
            && !self
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

    /// Checks the anchor offset for unsafe L2 blocks and triggers a reanchor if necessary.
    /// Returns true if reanchor was triggered.
    async fn check_and_handle_anchor_offset_for_unsafe_l2_blocks(
        &mut self,
        l2_slot_info: &L2SlotInfo,
    ) -> Result<bool, Error> {
        debug!("Checking anchor offset for unsafe L2 blocks to do fast reanchor when needed");
        let taiko_inbox_height = self
            .ethereum_l1
            .execution_layer
            .get_l2_height_from_taiko_inbox()
            .await?;
        if taiko_inbox_height < l2_slot_info.parent_id() {
            let l2_block_id = taiko_inbox_height + 1;
            let anchor_offset = self
                .batch_manager
                .get_l1_anchor_block_offset_for_l2_block(l2_block_id)
                .await?;
            let max_anchor_height_offset = self
                .taiko
                .get_protocol_config()
                .get_config_max_anchor_height_offset();

            // +1 because we are checking the next block
            if anchor_offset > max_anchor_height_offset + 1 {
                warn!(
                    "Anchor offset {} is too high for l2 block id {}, triggering reanchor",
                    anchor_offset, l2_block_id
                );
                if let Err(err) = self
                    .reanchor_blocks(
                        taiko_inbox_height,
                        "Anchor offset is too high for unsafe L2 blocks",
                        false,
                    )
                    .await
                {
                    error!("Failed to reanchor: {}", err);
                    self.cancel_token.cancel_on_critical_error();
                    return Err(anyhow::anyhow!("Failed to reanchor: {}", err));
                }
                return Ok(true);
            }
        }

        Ok(false)
    }

    async fn get_slot_info_and_status(
        &mut self,
    ) -> Result<(L2SlotInfo, OperatorStatus, Option<PreBuiltTxList>), Error> {
        let start = std::time::Instant::now();
        let l2_slot_info = self.taiko.get_l2_slot_info().await;
        let elapsed_l2_slot = start.elapsed();

        let current_status = match &l2_slot_info {
            Ok(info) => self.operator.get_status(info).await,
            Err(_) => Err(anyhow::anyhow!("Failed to get L2 slot info")),
        };
        let elapsed_status = start.elapsed();

        let batches_ready_to_send = self.batch_manager.get_number_of_batches_ready_to_send();
        let pending_tx_list = match &l2_slot_info {
            Ok(info) => {
                self.taiko
                    .get_pending_l2_tx_list_from_l2_engine(info.base_fee(), batches_ready_to_send)
                    .await
            }
            Err(_) => Err(anyhow::anyhow!("Failed to get L2 slot info")),
        };
        let elapsed_pending_tx = start.elapsed();

        self.print_current_slots_info(
            &current_status,
            &pending_tx_list,
            &l2_slot_info,
            self.batch_manager.get_number_of_batches(),
        )?;

        if elapsed_pending_tx.as_millis() > (self.config.preconf_heartbeat_ms / 2).into() {
            error!(
                "Critical error: getting slot info and status is taking too long: total {} ms (l2_slot_info: {} ms, status: {} ms, pending_tx_list: {} ms)",
                elapsed_pending_tx.as_millis(),
                elapsed_l2_slot.as_millis(),
                (elapsed_status - elapsed_l2_slot).as_millis(),
                (elapsed_pending_tx - elapsed_status).as_millis()
            );
            Self::log_status_durations(&current_status);
            return Err(anyhow::anyhow!(
                "Getting slot info and status is taking too long"
            ));
        }

        Ok((l2_slot_info?, current_status?, pending_tx_list?))
    }

    fn log_status_durations(current_status: &Result<OperatorStatus, Error>) {
        #[cfg(feature = "get_status_duration")]
        {
            if let Some(durations) = current_status.as_ref().ok().and_then(|s| s.get_durations()) {
                error!(
                    "Get status durations: check_taiko_wrapper: {} ms, check_handover_window_slots: {} ms, check_current_operator: {} ms, check_handover_window: {} ms, check_driver_status: {} ms, check_driver_synced: {} ms, check_preconfer: {} ms, check_preconfirmation_started: {} ms, check_submitter: {} ms, check_end_of_sequencing: {} ms",
                    durations.check_taiko_wrapper.as_millis(),
                    (durations.check_handover_window_slots - durations.check_taiko_wrapper)
                        .as_millis(),
                    (durations.check_current_operator - durations.check_handover_window_slots)
                        .as_millis(),
                    (durations.check_handover_window - durations.check_current_operator)
                        .as_millis(),
                    (durations.check_driver_status - durations.check_handover_window).as_millis(),
                    (durations.check_driver_synced - durations.check_driver_status).as_millis(),
                    (durations.check_preconfer - durations.check_driver_synced).as_millis(),
                    (durations.check_preconfirmation_started - durations.check_preconfer)
                        .as_millis(),
                    (durations.check_submitter - durations.check_preconfirmation_started)
                        .as_millis(),
                    (durations.check_end_of_sequencing - durations.check_submitter).as_millis(),
                );
            } else {
                error!("Get status durations not available (None)");
            }
        }
    }

    /// Returns true if the operation succeeds
    async fn has_verified_unproposed_batches(&mut self) -> Result<bool, Error> {
        if let Some(mut verifier) = self.verifier.take() {
            match verifier
                .verify(self.ethereum_l1.clone(), self.metrics.clone())
                .await
            {
                Ok(res) => match res {
                    VerificationResult::SlotNotValid => {
                        self.verifier = Some(verifier);
                        return Ok(false);
                    }
                    VerificationResult::ReanchorNeeded(block, reason) => {
                        if let Err(err) = self.reanchor_blocks(block, &reason, false).await {
                            error!("Failed to reanchor blocks: {}", err);
                            self.cancel_token.cancel_on_critical_error();
                            return Err(err);
                        }
                    }
                    VerificationResult::SuccessWithBatches(batches) => {
                        self.batch_manager.prepend_batches(batches);
                    }
                    VerificationResult::SuccessNoBatches => {}
                    VerificationResult::VerificationInProgress => {
                        self.verifier = Some(verifier);
                        return Ok(false);
                    }
                },
                Err(err) => {
                    self.verifier = Some(verifier);
                    return Err(err);
                }
            }
        }
        Ok(true)
    }

    async fn check_transaction_error_channel(
        &mut self,
        current_status: &OperatorStatus,
        l2_slot_info: &L2SlotInfo,
    ) -> Result<(), Error> {
        match self.transaction_error_channel.try_recv() {
            Ok(error) => {
                return self
                    .handle_transaction_error(&error, current_status, l2_slot_info)
                    .await;
            }
            Err(err) => match err {
                TryRecvError::Empty => {
                    // no errors, proceed with preconfirmation
                }
                TryRecvError::Disconnected => {
                    self.cancel_token.cancel_on_critical_error();
                    return Err(anyhow::anyhow!("Transaction error channel disconnected"));
                }
            },
        }

        Ok(())
    }

    async fn handle_transaction_error(
        &mut self,
        error: &TransactionError,
        current_status: &OperatorStatus,
        l2_slot_info: &L2SlotInfo,
    ) -> Result<(), Error> {
        match error {
            TransactionError::ReanchorRequired => {
                if current_status.is_preconfer() && current_status.is_submitter() {
                    let taiko_inbox_height = match self
                        .ethereum_l1
                        .execution_layer
                        .get_l2_height_from_taiko_inbox()
                        .await
                    {
                        Ok(height) => height,
                        Err(err) => {
                            error!(
                                "ReanchorRequired: Failed to get L2 height from Taiko inbox: {}",
                                err
                            );
                            self.cancel_token.cancel_on_critical_error();
                            return Err(anyhow::anyhow!(
                                "ReanchorRequired: Failed to get L2 height from Taiko inbox: {}",
                                err
                            ));
                        }
                    };
                    if let Err(err) = self
                        .reanchor_blocks(taiko_inbox_height, "Transaction reverted", false)
                        .await
                    {
                        error!("ReanchorRequired: Failed to reanchor blocks: {}", err);
                        self.cancel_token.cancel_on_critical_error();
                        return Err(anyhow::anyhow!(
                            "ReanchorRequired: Failed to reanchor blocks: {}",
                            err
                        ));
                    }
                    return Err(anyhow::anyhow!("Reanchoring done"));
                } else {
                    warn!("Transaction reverted, not our epoch, skipping reorg");
                }
            }
            TransactionError::NotConfirmed => {
                self.cancel_token.cancel_on_critical_error();
                return Err(anyhow::anyhow!(
                    "Transaction not confirmed for a long time, exiting"
                ));
            }
            TransactionError::UnsupportedTransactionType => {
                self.cancel_token.cancel_on_critical_error();
                return Err(anyhow::anyhow!(
                    "Unsupported transaction type. You can send eip1559 or eip4844 transactions only"
                ));
            }
            TransactionError::GetBlockNumberFailed => {
                // TODO recreate L1 provider
                self.cancel_token.cancel_on_critical_error();
                return Err(anyhow::anyhow!("Failed to get block number from L1"));
            }
            TransactionError::EstimationTooEarly => {
                warn!("Transaction estimation too early");
                return Ok(());
            }
            TransactionError::InsufficientFunds => {
                self.cancel_token.cancel_on_critical_error();
                return Err(anyhow::anyhow!(
                    "Transaction reverted with InsufficientFunds error"
                ));
            }
            TransactionError::EstimationFailed => {
                self.cancel_token.cancel_on_critical_error();
                return Err(anyhow::anyhow!("Transaction estimation failed, exiting"));
            }
            TransactionError::TransactionReverted => {
                self.cancel_token.cancel_on_critical_error();
                return Err(anyhow::anyhow!("Transaction reverted, exiting"));
            }
            TransactionError::OldestForcedInclusionDue => {
                self.metrics.inc_critical_errors();
                let taiko_inbox_height = match self
                    .ethereum_l1
                    .execution_layer
                    .get_l2_height_from_taiko_inbox()
                    .await
                {
                    Ok(height) => height,
                    Err(err) => {
                        let err_msg = format!(
                            "OldestForcedInclusionDue: Failed to get L2 height from Taiko inbox: {err}"
                        );
                        error!("{}", err_msg);
                        self.cancel_token.cancel_on_critical_error();
                        return Err(anyhow::anyhow!("{}", err_msg));
                    }
                };
                if let Err(err) = self
                    .handle_oldest_forced_inclusion_due(
                        taiko_inbox_height,
                        current_status,
                        l2_slot_info,
                    )
                    .await
                {
                    let err_msg = format!("OldestForcedInclusionDue: Failed to reorg: {err}");
                    error!("{}", err_msg);
                    self.cancel_token.cancel_on_critical_error();
                    return Err(anyhow::anyhow!("{}", err_msg));
                }
                return Err(anyhow::anyhow!(
                    "Need to include forced inclusion, reanchoring done, skipping slot"
                ));
            }
            TransactionError::NotTheOperatorInCurrentEpoch => {
                warn!("Propose batch transaction executed too late.");
                return Ok(());
            }
        }

        Ok(())
    }

    async fn preconfirm_block(
        &mut self,
        pending_tx_list: Option<PreBuiltTxList>,
        l2_slot_info: &L2SlotInfo,
        end_of_sequencing: bool,
        allow_forced_inclusion: bool,
    ) -> Result<Option<BuildPreconfBlockResponse>, Error> {
        let result = self
            .batch_manager
            .preconfirm_block(
                pending_tx_list,
                l2_slot_info,
                end_of_sequencing,
                allow_forced_inclusion,
            )
            .await?;
        Ok(result)
    }

    fn print_current_slots_info(
        &self,
        current_status: &Result<OperatorStatus, Error>,
        pending_tx_list: &Result<Option<PreBuiltTxList>, Error>,
        l2_slot_info: &Result<L2SlotInfo, Error>,
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

    async fn handle_oldest_forced_inclusion_due(
        &mut self,
        parent_block_id: u64,
        current_status: &OperatorStatus,
        l2_slot_info: &L2SlotInfo,
    ) -> Result<(), Error> {
        warn!(
            "⛓️‍💥 OldestForcedInclusionDue Error. Reorg to parent block: {} and add forced inclusion",
            parent_block_id
        );

        // Update self state
        self.verifier = None;
        self.batch_manager.reset_builder().await?;

        self.chain_monitor.set_expected_reorg(parent_block_id).await;

        if self
            .batch_manager
            .add_new_l2_block_with_forced_inclusion(OperationType::Reanchor, l2_slot_info)
            .await?
            .is_none()
        {
            let err_msg = format!(
                "Failed to reorg to parent block: {parent_block_id} and add forced inclusion",
            );
            error!("{}", err_msg);
            return Err(anyhow::anyhow!("{}", err_msg));
        }

        // If now is an end of sequencing block we must preconfirm the block
        if current_status.is_end_of_sequencing() {
            // TODO
        }

        Ok(())
    }

    async fn reanchor_blocks(
        &mut self,
        parent_block_id: u64,
        reason: &str,
        allow_forced_inclusion: bool,
    ) -> Result<(), Error> {
        warn!(
            "⛓️‍💥 Reanchoring blocks for parent block: {} reason: {} allow_forced_inclusion: {}",
            parent_block_id, reason, allow_forced_inclusion
        );

        let start_time = std::time::Instant::now();

        let mut l2_slot_info = self
            .taiko
            .get_l2_slot_info_by_parent_block(alloy::eips::BlockNumberOrTag::Number(
                parent_block_id,
            ))
            .await?;

        // Update self state
        self.verifier = None;
        self.batch_manager.reset_builder().await?;

        self.chain_monitor.set_expected_reorg(parent_block_id).await;

        let start_block_id = parent_block_id + 1;
        let blocks = self
            .taiko
            .fetch_l2_blocks_until_latest(start_block_id, true)
            .await?;

        let blocks_reanchored = blocks.len() as u64;

        let mut forced_inclusion_flags: Vec<bool> = Vec::with_capacity(blocks.len());
        for block in &blocks {
            forced_inclusion_flags.push(
                self.batch_manager
                    .is_forced_inclusion(block.header.number)
                    .await?,
            );
        }

        for (block, is_forced_inclusion) in blocks.iter().zip(forced_inclusion_flags) {
            debug!(
                "Reanchoring block {} with {} transactions, parent_id {}, parent_hash {}, is_forced_inclusion: {}",
                block.header.number,
                block.transactions.len(),
                l2_slot_info.parent_id(),
                l2_slot_info.parent_hash(),
                is_forced_inclusion,
            );

            let (_, txs) = match block.transactions.as_transactions() {
                Some(txs) => txs.split_first().ok_or_else(|| {
                    anyhow::anyhow!(
                        "Cannot get anchor transaction from block {}",
                        block.header.number
                    )
                })?,
                None => {
                    return Err(anyhow::anyhow!(
                        "No transactions in block {}",
                        block.header.number
                    ));
                }
            };

            let tx_list = txs.to_vec();
            let bytes_length =
                crate::shared::l2_tx_lists::encode_and_compress(&tx_list)?.len() as u64;
            let pending_tx_list = crate::shared::l2_tx_lists::PreBuiltTxList {
                tx_list,
                estimated_gas_used: 0,
                bytes_length,
            };

            let block = self
                .batch_manager
                .reanchor_block(
                    pending_tx_list,
                    &l2_slot_info,
                    is_forced_inclusion,
                    allow_forced_inclusion,
                )
                .await;
            // if reanchor_block fails restart the node
            match block {
                Ok(block) => {
                    debug!("Reanchored block {} hash {}", block.number, block.hash);
                }
                Err(err) => {
                    let err_msg = format!("Failed to reanchor block: {err}");
                    error!("{}", err_msg);
                    self.cancel_token.cancel_on_critical_error();
                    return Err(anyhow::anyhow!("{}", err_msg));
                }
            }

            // TODO reduce 1 geth call
            // We can get previous L2 slot info from BuildPreconfBlockResponse
            l2_slot_info = self.taiko.get_l2_slot_info().await?;
        }

        self.head_verifier
            .set(l2_slot_info.parent_id(), *l2_slot_info.parent_hash())
            .await;

        self.metrics.inc_by_blocks_reanchored(blocks_reanchored);

        debug!(
            "Finished reanchoring blocks for parent block {} in {} ms",
            parent_block_id,
            start_time.elapsed().as_millis()
        );
        Ok(())
    }
}
