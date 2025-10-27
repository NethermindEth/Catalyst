use std::sync::Arc;

use anyhow::Error;
use common::{
    l1::{ethereum_l1::EthereumL1, traits::ELTrait},
    l2::taiko_driver::OperationType,
    shared::{l2_block::L2Block, l2_slot_info::L2SlotInfo, l2_tx_lists::PreBuiltTxList},
    utils as common_utils,
};
use pacaya::node::config::NodeConfig;
use tokio::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};

use pacaya::node::operator::Status as OperatorStatus;

use crate::{l1::execution_layer::ExecutionLayer, l2::taiko::Taiko};

pub struct Node {
    config: NodeConfig,
    cancel_token: CancellationToken,
    ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
    taiko: Arc<Taiko>,
    watchdog: common_utils::watchdog::Watchdog,
}

impl Node {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        config: NodeConfig,
        cancel_token: CancellationToken,
        ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
        taiko: Arc<Taiko>,
    ) -> Result<Self, Error> {
        let watchdog = common_utils::watchdog::Watchdog::new(
            cancel_token.clone(),
            ethereum_l1.slot_clock.get_l2_slots_per_epoch() / 2,
        );
        Ok(Self {
            config,
            cancel_token,
            ethereum_l1,
            taiko,
            watchdog,
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

    async fn main_block_preconfirmation_step(&mut self) -> Result<(), Error> {
        let (l2_slot_info, current_status, pending_tx_list) =
            self.get_slot_info_and_status().await?;

        // TODO preconfirmation
        if current_status.is_preconfer()
            && let Some(pending_tx_list) = pending_tx_list
        {
            let l2_block = L2Block::new_from(
                pending_tx_list,
                std::time::SystemTime::now() // temp solution
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_secs(),
            );
            let anchor_height = self
                .ethereum_l1
                .execution_layer
                .common()
                .get_latest_block_id()
                .await?
                - self.config.l1_height_lag;

            let anchor_hash = self
                .ethereum_l1
                .execution_layer
                .common()
                .get_block_hash(anchor_height)
                .await?;

            let anchor_block_state_root = self
                .ethereum_l1
                .execution_layer
                .common()
                .get_block_state_root_by_number(anchor_height)
                .await?;

            self.taiko
                .advance_head_to_new_l2_block(
                    l2_block,
                    anchor_height,
                    anchor_block_state_root,
                    anchor_hash,
                    &l2_slot_info,
                    false,
                    false,
                    OperationType::Preconfirm,
                )
                .await?;
        }

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
