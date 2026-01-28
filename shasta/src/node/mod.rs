pub mod proposal_manager;
use anyhow::Error;
use common::{
    fork_info::ForkInfo,
    l1::{ethereum_l1::EthereumL1, transaction_error::TransactionError},
    l2::taiko_driver::{TaikoDriver, models::BuildPreconfBlockResponse},
    shared::{l2_slot_info_v2::L2SlotContext, l2_tx_lists::PreBuiltTxList},
    utils::{self as common_utils, cancellation_token::CancellationToken},
};
use pacaya::node::operator::Status as OperatorStatus;
use pacaya::node::{config::NodeConfig, operator::Operator};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::metrics::Metrics;
use crate::{l1::execution_layer::ExecutionLayer, l2::taiko::Taiko};
use common::batch_builder::BatchBuilderConfig;
use common::l1::traits::PreconferProvider;
use common::shared::head_verifier::HeadVerifier;
use common::shared::l2_slot_info_v2::L2SlotInfoV2;
use proposal_manager::BatchManager;

use tokio::{
    sync::mpsc::{Receiver, error::TryRecvError},
    time::{Duration, sleep},
};

mod verifier;
use verifier::{VerificationResult, Verifier};

mod l2_height_from_l1;
use crate::chain_monitor::ShastaChainMonitor;
pub use l2_height_from_l1::get_l2_height_from_l1;

pub struct Node {
    config: NodeConfig,
    cancel_token: CancellationToken,
    ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
    taiko: Arc<Taiko>,
    watchdog: common_utils::watchdog::Watchdog,
    operator: Operator<ExecutionLayer, common::l1::slot_clock::RealClock, TaikoDriver>,
    metrics: Arc<Metrics>,
    proposal_manager: BatchManager, //TODO change name or unify with pacaya's batch manager
    verifier: Option<Verifier>,
    head_verifier: HeadVerifier,
    transaction_error_channel: Receiver<TransactionError>,
    chain_monitor: Arc<ShastaChainMonitor>,
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
        chain_monitor: Arc<ShastaChainMonitor>,
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
            //TODO
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

        Ok(Self {
            config,
            cancel_token,
            ethereum_l1,
            taiko,
            watchdog,
            operator,
            metrics,
            proposal_manager,
            verifier: None,
            head_verifier,
            transaction_error_channel,
            chain_monitor,
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

    async fn main_block_preconfirmation_step(&mut self) -> Result<(), Error> {
        let (l2_slot_info, current_status, pending_tx_list) =
            self.get_slot_info_and_status().await?;

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
                    self.proposal_manager
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

            let l2_slot_context = L2SlotContext {
                // TODO remove clone
                info: l2_slot_info.clone(),
                end_of_sequencing: current_status.is_end_of_sequencing(),
                allow_forced_inclusion: self.config.propose_forced_inclusion
                    && current_status.is_submitter()
                    && self.verifier.is_none(),
            };

            if self
                .proposal_manager
                .should_new_block_be_created(&pending_tx_list, &l2_slot_context)
            {
                let preconfed_block = self
                    .proposal_manager
                    .preconfirm_block(pending_tx_list, &l2_slot_context)
                    .await?;

                self.verify_preconfed_block(preconfed_block).await?;
            }
        }

        if current_status.is_submitter() && !transaction_in_progress {
            // first check verifier
            if self.has_verified_unproposed_batches().await?
                && let Err(err) = self
                    .proposal_manager
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
            if self.proposal_manager.has_batches()
                || self.proposal_manager.has_current_forced_inclusion()
            {
                error!(
                    "Resetting batch builder. has batches: {}, has current forced inclusion: {}",
                    self.proposal_manager.has_batches(),
                    self.proposal_manager.has_current_forced_inclusion()
                );
                self.proposal_manager.reset_builder().await?;
            }
            if self.verifier.is_some() {
                error!("Verifier is not None after submitter window.");
                self.verifier = None;
            }
        }

        Ok(())
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
                        self.proposal_manager
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

    /// Returns true if the operation succeeds
    /// Returns true if the operation succeeds
    async fn has_verified_unproposed_batches(&mut self) -> Result<bool, Error> {
        if let Some(mut verifier) = self.verifier.take() {
            match verifier
                .verify(
                    self.ethereum_l1.clone(),
                    self.taiko.clone(),
                    self.metrics.clone(),
                )
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
                        self.proposal_manager.prepend_batches(batches);
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
                Err(anyhow::anyhow!(
                    "Unsupported transaction type. You can send eip1559 or eip4844 transactions only"
                ))
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
                // TODO implement proper handling of forced inclusion due
                self.cancel_token.cancel_on_critical_error();
                Err(anyhow::anyhow!(
                    "Need to include forced inclusion, reanchoring done, skipping slot"
                ))
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
                error!("Failed to get L2 slot info set  gas_limit_without_anchor to 0");
                0u64
            }
        };

        let pending_tx_list = if gas_limit_without_anchor != 0 {
            // TODO use proper number of batches ready to send
            let batches_ready_to_send = 0; //self.batch_manager.get_number_of_batches_ready_to_send();
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

    /// Checks the anchor offset for unsafe L2 blocks and triggers a reanchor if necessary.
    /// Returns true if reanchor was triggered.
    async fn check_and_handle_anchor_offset_for_unsafe_l2_blocks(
        &mut self,
        l2_slot_info: &L2SlotInfoV2,
    ) -> Result<bool, Error> {
        debug!("Checking anchor offset for unsafe L2 blocks to do fast reanchor when needed");
        let taiko_inbox_height =
            get_l2_height_from_l1(self.ethereum_l1.clone(), self.taiko.clone()).await?;
        if taiko_inbox_height < l2_slot_info.parent_id() {
            let l2_block_id = taiko_inbox_height + 1;
            let anchor_offset = self
                .proposal_manager
                .get_l1_anchor_block_offset_for_l2_block(l2_block_id)
                .await?;
            let max_anchor_height_offset = self
                .taiko
                .get_protocol_config()
                .get_max_anchor_height_offset();

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

    async fn get_current_protocol_height(&self) -> Result<(u64, u64), Error> {
        let taiko_inbox_height =
            get_l2_height_from_l1(self.ethereum_l1.clone(), self.taiko.clone()).await?;

        let taiko_geth_height = self.taiko.get_latest_l2_block_id().await?;

        Ok((taiko_inbox_height, taiko_geth_height))
    }

    async fn get_next_proposal_id(&self) -> Result<(u64, u64), Error> {
        let l1_proposal_id = self
            .ethereum_l1
            .execution_layer
            .get_inbox_next_proposal_id()
            .await?;

        let l2_proposal_id = self
            .taiko
            .l2_execution_layer()
            .get_last_synced_proposal_id_from_geth()
            .await
            .unwrap_or(0)
            + 1;

        Ok((l1_proposal_id, l2_proposal_id))
    }

    async fn check_transaction_error_channel(
        &mut self,
        current_status: &OperatorStatus,
        l2_slot_info: &L2SlotInfoV2,
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
        info!("Warmup node");

        // Wait for Inbox activation
        let mut activation_timestamp = self
            .ethereum_l1
            .execution_layer
            .get_activation_timestamp()
            .await?;

        while activation_timestamp == 0 {
            warn!(
                "Shasta Inbox is not activated yet. Waiting {} seconds...",
                self.ethereum_l1.slot_clock.get_slot_duration().as_secs()
            );
            sleep(self.ethereum_l1.slot_clock.get_slot_duration()).await;
            activation_timestamp = self
                .ethereum_l1
                .execution_layer
                .get_activation_timestamp()
                .await?;
        }

        // Wait for Taiko Geth to synchronize with L1
        loop {
            let (l1_proposal_id, l2_proposal_id) = self.get_next_proposal_id().await?;
            info!(
                "Inbox next proposal id: {l1_proposal_id}, Taiko Geth next proposal id: {l2_proposal_id}"
            );

            if l1_proposal_id <= l2_proposal_id {
                break;
            }

            warn!(
                "Taiko Geth is behind L1 (L1: {l1_proposal_id}, L2: {l2_proposal_id}). Retrying in 5 seconds..."
            );
            sleep(Duration::from_secs(5)).await;
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

        // Update self state
        self.verifier = None;
        self.proposal_manager.reset_builder().await?;

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
                self.proposal_manager
                    .is_forced_inclusion(block.header.number)
                    .await?,
            );
        }

        self.reanchor_blocks_internal(
            &blocks,
            &forced_inclusion_flags,
            start_block_id,
            parent_block_id,
            allow_forced_inclusion,
        )
        .await?;

        let last_l2_slot_info = self.taiko.get_l2_slot_info().await?;
        self.head_verifier
            .set(
                last_l2_slot_info.parent_id(),
                *last_l2_slot_info.parent_hash(),
            )
            .await;

        self.metrics.inc_by_blocks_reanchored(blocks_reanchored);

        debug!(
            "Finished reanchoring blocks for parent block {} in {} ms",
            parent_block_id,
            start_time.elapsed().as_millis()
        );
        Ok(())
    }

    async fn reanchor_blocks_internal(
        &mut self,
        blocks: &[alloy::rpc::types::Block],
        forced_inclusion_flags: &[bool],
        start_block_id: u64,
        parent_block_id: u64,
        allow_forced_inclusion: bool,
    ) -> Result<(), Error> {
        for (block, is_forced_inclusion) in blocks.iter().zip(forced_inclusion_flags) {
            let l2_slot_info = if block.header.number == start_block_id {
                self.taiko
                    .get_l2_slot_info_by_parent_block(alloy::eips::BlockNumberOrTag::Number(
                        parent_block_id,
                    ))
                    .await?
            } else {
                self.taiko.get_l2_slot_info().await?
            };
            // use block timestamp as l2 slot timestamp,
            // otherwise it can be the same as the previous block
            // which is forbidden by the protocol
            let l2_slot_info = L2SlotInfoV2::new_from_other(l2_slot_info, block.header.timestamp);

            debug!(
                "Reanchoring block {} with {} transactions, parent_id {}, parent_hash {}, is_forced_inclusion: {}, timestamp: {}",
                block.header.number,
                block.transactions.len(),
                l2_slot_info.parent_id(),
                l2_slot_info.parent_hash(),
                is_forced_inclusion,
                l2_slot_info.slot_timestamp(),
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

            let pending_tx_list = crate::shared::l2_tx_lists::PreBuiltTxList {
                tx_list: txs.to_vec(),
                estimated_gas_used: 0,
                bytes_length: 0,
            };

            // if reanchor_block fails restart the node
            match self
                .proposal_manager
                .reanchor_block(
                    pending_tx_list,
                    l2_slot_info,
                    *is_forced_inclusion,
                    allow_forced_inclusion,
                )
                .await
            {
                Ok(reanchored_block) => {
                    debug!(
                        "Reanchored block {} hash {}",
                        reanchored_block.number, reanchored_block.hash
                    );
                }
                Err(err) => {
                    let err_msg =
                        format!("Failed to reanchor block {}: {}", block.header.number, err);
                    error!("{}", err_msg);
                    self.cancel_token.cancel_on_critical_error();
                    return Err(anyhow::anyhow!("{}", err_msg));
                }
            }
        }

        Ok(())
    }
}
