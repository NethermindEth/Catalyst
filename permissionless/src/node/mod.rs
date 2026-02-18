use crate::node::operator::status::Status as OperatorStatus;
use crate::node::{config::NodeConfig, operator::Operator};
use anyhow::Error;
use common::{
    l1::{ethereum_l1::EthereumL1, transaction_error::TransactionError},
    metrics::Metrics,
    shared::{l2_slot_info_v2::L2SlotInfoV2, l2_tx_lists::PreBuiltTxList},
    utils::{self as common_utils, cancellation_token::CancellationToken},
};
use shasta::{
    ProposalManager, l1::execution_layer::ExecutionLayer as ShastaExecutionLayer, l2::taiko::Taiko,
};
use std::sync::Arc;
use tokio::{sync::mpsc::Receiver, time::Duration};
use tracing::{debug, error, info};

pub mod config;
pub mod operator;

pub struct Node {
    cancel_token: CancellationToken,
    ethereum_l1: Arc<EthereumL1<ShastaExecutionLayer>>,
    _transaction_error_channel: Receiver<TransactionError>,
    _metrics: Arc<Metrics>,
    watchdog: common_utils::watchdog::Watchdog,
    config: NodeConfig,
    operator: Operator,
    proposal_manager: ProposalManager,
    taiko: Arc<Taiko>,
}

impl Node {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cancel_token: CancellationToken,
        ethereum_l1: Arc<EthereumL1<ShastaExecutionLayer>>,
        transaction_error_channel: Receiver<TransactionError>,
        metrics: Arc<Metrics>,
        config: NodeConfig,
        operator: Operator,
        proposal_manager: ProposalManager,
        taiko: Arc<Taiko>,
    ) -> Result<Self, Error> {
        let watchdog = common_utils::watchdog::Watchdog::new(
            cancel_token.clone(),
            ethereum_l1.slot_clock.get_l2_slots_per_epoch() / 2,
        );
        Ok(Self {
            cancel_token,
            ethereum_l1,
            _transaction_error_channel: transaction_error_channel,
            _metrics: metrics,
            watchdog,
            config,
            operator,
            proposal_manager,
            taiko,
        })
    }

    pub async fn entrypoint(mut self) -> Result<(), Error> {
        info!("Starting node");

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
        let (_l2_slot_info, _current_status, _pending_tx_list) =
            self.get_slot_info_and_status().await?;

        Ok(())
    }

    async fn get_slot_info_and_status(
        &mut self,
    ) -> Result<(L2SlotInfoV2, OperatorStatus, Option<PreBuiltTxList>), Error> {
        let l2_slot_info = self.taiko.get_l2_slot_info().await;
        let current_status = match &l2_slot_info {
            Ok(info) => self.operator.get_status(info.clone()).await,
            Err(_) => Err(anyhow::anyhow!("Failed to get L2 slot info")),
        };

        let gas_limit_without_anchor = match &l2_slot_info {
            Ok(info) => info.parent_gas_limit_without_anchor(),
            Err(_) => {
                error!("Failed to get L2 slot info; set gas_limit_without_anchor to 0");
                0u64
            }
        };

        let pending_tx_list = if gas_limit_without_anchor != 0 {
            let proposals_ready_to_send = self
                .proposal_manager
                .get_number_of_proposals_ready_to_send();
            match &l2_slot_info {
                Ok(info) => {
                    self.taiko
                        .get_pending_l2_tx_list_from_l2_engine(
                            info.base_fee(),
                            proposals_ready_to_send,
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
            self.proposal_manager.get_number_of_proposals(),
        )?;

        Ok((l2_slot_info?, current_status?, pending_tx_list?))
    }

    fn print_current_slots_info(
        &self,
        current_status: &Result<OperatorStatus, Error>,
        pending_tx_list: &Result<Option<PreBuiltTxList>, Error>,
        l2_slot_info: &Result<L2SlotInfoV2, Error>,
        proposals_number: u64,
    ) -> Result<(), Error> {
        let l1_slot = self.ethereum_l1.slot_clock.get_current_slot()?;
        info!(target: "heartbeat",
            "| Epoch: {:<6} | Slot: {:<2} | L2 Slot: {:<2} | {}{} Proposals: {proposals_number} | {} |",
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
