pub mod config;
pub mod consensus_layer;
pub mod execution_layer;
pub mod execution_layer_inner;
pub mod extension;
pub mod forced_inclusion_info;
pub mod l1_contracts_bindings;
mod monitor_transaction;
pub mod propose_batch_builder; // TODO: move to the whitelist module
pub mod slot_clock;
mod tools;
pub mod transaction_error;

use anyhow::Error;
use config::EthereumL1Config;
use consensus_layer::ConsensusLayer;
use execution_layer::ExecutionLayer;
use extension::ELExtension;
use slot_clock::SlotClock;
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc::Sender;
use transaction_error::TransactionError;

use crate::metrics::Metrics;

pub struct EthereumL1<T: ELExtension> {
    pub slot_clock: Arc<SlotClock>,
    pub consensus_layer: ConsensusLayer,
    pub execution_layer: Arc<ExecutionLayer<T>>,
}

impl<T: ELExtension> EthereumL1<T> {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        config: EthereumL1Config,
        specific_config: T::Config,
        transaction_error_channel: Sender<TransactionError>,
        metrics: Arc<Metrics>,
    ) -> Result<Self, Error> {
        tracing::info!("Creating EthereumL1 instance");
        let consensus_layer = ConsensusLayer::new(
            &config.consensus_rpc_url,
            Duration::from_millis(config.preconf_heartbeat_ms / 2),
        )?;
        let genesis_time = consensus_layer.get_genesis_time().await?;
        let slot_clock = Arc::new(SlotClock::new(
            0u64,
            genesis_time,
            config.slot_duration_sec,
            config.slots_per_epoch,
            config.preconf_heartbeat_ms,
        ));

        let execution_layer =
            ExecutionLayer::new(config, specific_config, transaction_error_channel, metrics)
                .await?;

        Ok(Self {
            slot_clock,
            consensus_layer,
            execution_layer: Arc::new(execution_layer),
        })
    }
}
