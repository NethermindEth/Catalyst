use crate::{l1::execution_layer::ExecutionLayer, node::config::NodeConfig};
use anyhow::Error;
use common::{
    l1::{ethereum_l1::EthereumL1, transaction_error::TransactionError},
    metrics::Metrics,
    utils as common_utils,
    utils::cancellation_token::CancellationToken,
};
use std::sync::Arc;
use tokio::{sync::mpsc::Receiver, time::Duration};
use tracing::{debug, error, info};

pub mod config;

pub struct Node {
    cancel_token: CancellationToken,
    ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
    _transaction_error_channel: Receiver<TransactionError>,
    _metrics: Arc<Metrics>,
    watchdog: common_utils::watchdog::Watchdog,
    config: NodeConfig,
}

impl Node {
    pub fn new(
        cancel_token: CancellationToken,
        ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
        transaction_error_channel: Receiver<TransactionError>,
        metrics: Arc<Metrics>,
        config: NodeConfig,
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
        self.print_current_slots_info()?;
        Ok(())
    }

    fn print_current_slots_info(&self) -> Result<(), Error> {
        let l1_slot = self.ethereum_l1.slot_clock.get_current_slot()?;
        info!(target: "heartbeat",
            "| Epoch: {:<6} | Slot: {:<2} | L2 Slot: {:<2} |",
            self.ethereum_l1.slot_clock.get_epoch_from_slot(l1_slot),
            self.ethereum_l1.slot_clock.slot_of_epoch(l1_slot),
            self.ethereum_l1
                .slot_clock
                .get_current_l2_slot_within_l1_slot()?,
        );
        Ok(())
    }
}
