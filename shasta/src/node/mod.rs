pub mod proposal_manager;
use std::sync::Arc;

use anyhow::Error;
use common::{
    l1::{ethereum_l1::EthereumL1, transaction_error::TransactionError},
    l2::taiko_driver::{TaikoDriver, models::BuildPreconfBlockResponse},
    shared::{l2_slot_info::L2SlotInfo, l2_tx_lists::PreBuiltTxList},
    utils as common_utils,
};
use pacaya::node::operator::Status as OperatorStatus;
use pacaya::node::{config::NodeConfig, operator::Operator};
use tokio::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};

use crate::metrics::Metrics;
use crate::{
    l1::{event_indexer::EventIndexer, execution_layer::ExecutionLayer},
    l2::taiko::Taiko,
};
use pacaya::node::batch_manager::config::BatchBuilderConfig;
use proposal_manager::BatchManager;

pub struct Node {
    config: NodeConfig,
    cancel_token: CancellationToken,
    ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
    taiko: Arc<Taiko>,
    watchdog: common_utils::watchdog::Watchdog,
    operator: Operator<ExecutionLayer, common::l1::slot_clock::RealClock, TaikoDriver>,
    event_indexer: Arc<EventIndexer>,
    metrics: Arc<Metrics>,
    proposal_manager: BatchManager, //TODO
}

impl Node {
    pub async fn new(
        config: NodeConfig,
        cancel_token: CancellationToken,
        ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
        taiko: Arc<Taiko>,
        event_indexer: Arc<EventIndexer>,
        metrics: Arc<Metrics>,
        batch_builder_config: BatchBuilderConfig,
    ) -> Result<Self, Error> {
        let operator = Operator::new(
            ethereum_l1.execution_layer.clone(),
            ethereum_l1.slot_clock.clone(),
            taiko.get_driver(),
            config.handover_window_slots,
            config.handover_start_buffer_ms,
            config.simulate_not_submitting_at_the_end_of_epoch,
            cancel_token.clone(),
        )
        .map_err(|e| anyhow::anyhow!("Failed to create Operator: {}", e))?;
        let watchdog = common_utils::watchdog::Watchdog::new(
            cancel_token.clone(),
            ethereum_l1.slot_clock.get_l2_slots_per_epoch() / 2,
        );

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

        Ok(Self {
            config,
            cancel_token,
            ethereum_l1,
            taiko,
            watchdog,
            operator,
            event_indexer,
            metrics,
            proposal_manager,
        })
    }

    pub async fn entrypoint(mut self) -> Result<(), Error> {
        info!("Starting node");

        // TODO
        /*if let Err(err) = self.warmup().await {
            error!("Failed to warm up node: {}. Shutting down.", err);
            self.cancel_token.cancel();
            return Err(anyhow::anyhow!(err));
        }

        info!("Node warmup successful");*/

        // Run preconfirmation loop in background
        tokio::spawn(async move {
            self.preconfirmation_loop().await;
        });

        Ok(())
    }

    async fn preconfirmation_loop(&mut self) {
        debug!("Main perconfirmation loop started");
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

    async fn preconfirm_block(
        &mut self,
        pending_tx_list: Option<PreBuiltTxList>,
        l2_slot_info: &L2SlotInfo,
        end_of_sequencing: bool,
        allow_forced_inclusion: bool,
    ) -> Result<Option<BuildPreconfBlockResponse>, Error> {
        let result = self
            .proposal_manager
            .preconfirm_block(
                pending_tx_list,
                l2_slot_info,
                end_of_sequencing,
                allow_forced_inclusion,
            )
            .await?;
        Ok(result)
    }

    async fn main_block_preconfirmation_step(&mut self) -> Result<(), Error> {
        let (l2_slot_info, current_status, pending_tx_list) =
            self.get_slot_info_and_status().await?;

        // TODO preconfirmation
        if current_status.is_preconfer() && current_status.is_driver_synced() {
            let _preconfed_block = self
                .preconfirm_block(
                    pending_tx_list,
                    &l2_slot_info,
                    current_status.is_end_of_sequencing(),
                    false, // TODO preconf FI
                )
                .await?;

            // TODO fix verification
            // self.verify_preconfed_block(preconfed_block).await?;
        }

        // TODO Get the transaction status before checking the error channel
        // to avoid race condition
        let transaction_in_progress = self
            .ethereum_l1
            .execution_layer
            .is_transaction_in_progress()
            .await?;

        if current_status.is_submitter() && !transaction_in_progress {
            // first check verifier
            if self.has_verified_unproposed_batches().await?
                && let Err(err) = self
                    .proposal_manager
                    .try_submit_oldest_batch(
                        current_status.is_preconfer(),
                        self.event_indexer.clone(),
                    )
                    .await
            {
                if let Some(transaction_error) = err.downcast_ref::<TransactionError>() {
                    self.handle_transaction_error(
                        transaction_error,
                        &current_status,
                        &l2_slot_info,
                    )
                    .await?;
                }
                return Err(err);
            }
        }

        Ok(())
    }

    /// Returns true if the operation succeeds
    async fn has_verified_unproposed_batches(&mut self) -> Result<bool, Error> {
        // TODO implement proper verification
        Ok(true)
    }

    // TODO handle transaction error properly
    async fn handle_transaction_error(
        &mut self,
        error: &TransactionError,
        _current_status: &OperatorStatus,
        _l2_slot_info: &L2SlotInfo,
    ) -> Result<(), Error> {
        info!("Handling transaction error: {error}");
        Ok(())
    }

    async fn get_slot_info_and_status(
        &mut self,
    ) -> Result<(L2SlotInfo, OperatorStatus, Option<PreBuiltTxList>), Error> {
        let l2_slot_info = self.taiko.get_l2_slot_info().await;
        let current_status = match &l2_slot_info {
            // TODO fix operator status
            Ok(_info) => Ok(OperatorStatus::new(true, false, true, false, false)), // self.operator.get_status(info).await,
            Err(_) => Err(anyhow::anyhow!("Failed to get L2 slot info")),
        };
        // TODO use proper number of batches ready to send
        let batches_ready_to_send = 0; // self.batch_manager.get_number_of_batches_ready_to_send();
        let pending_tx_list = match &l2_slot_info {
            Ok(info) => {
                self.taiko
                    .get_pending_l2_tx_list_from_l2_engine(info.base_fee(), batches_ready_to_send)
                    .await
            }
            Err(_) => Err(anyhow::anyhow!("Failed to get L2 slot info")),
        };
        self.print_current_slots_info(
            &current_status,
            &pending_tx_list,
            &l2_slot_info,
            // TODO use proper number of batches
            //self.batch_manager.get_number_of_batches(),
            0,
        )?;

        Ok((l2_slot_info?, current_status?, pending_tx_list?))
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
}
