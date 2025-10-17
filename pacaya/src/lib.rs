use crate::utils::config::PacayaConfig;
use anyhow::Error;
use common::{
    config::ConfigTrait,
    funds_controller::FundsController,
    l1::{self as common_l1, traits::preconfer_provider::PreconferProvider},
    metrics::{self, Metrics},
    shared,
};
use l1::execution_layer::ExecutionLayer;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

mod chain_monitor;
mod forced_inclusion;
pub mod l1;
mod l2;
mod node;
pub mod utils;

pub async fn create_pacaya_node(
    config: common::config::Config,
    metrics: Arc<Metrics>,
    cancel_token: CancellationToken,
    switch_timestamp: Option<u64>,
) -> Result<(), Error> {
    // Read specific config from environment variables
    let pacaya_config = PacayaConfig::read_env_variables();

    let (transaction_error_sender, transaction_error_receiver) = mpsc::channel(100);
    let ethereum_l1 = common_l1::ethereum_l1::EthereumL1::<ExecutionLayer>::new(
        common_l1::config::EthereumL1Config::new(&config).await?,
        l1::config::EthereumL1Config::try_from(pacaya_config.clone())?,
        transaction_error_sender,
        metrics.clone(),
    )
    .await
    .map_err(|e| anyhow::anyhow!("Failed to create EthereumL1: {}", e))?;

    let ethereum_l1 = Arc::new(ethereum_l1);

    let taiko_config = l2::config::TaikoConfig::new(&config)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create TaikoConfig: {}", e))?;

    let taiko = Arc::new(
        l2::taiko::Taiko::new(ethereum_l1.clone(), metrics.clone(), taiko_config)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create Taiko: {}", e))?,
    );

    let max_anchor_height_offset = ethereum_l1
        .execution_layer
        .get_config_max_anchor_height_offset();
    if config.max_anchor_height_offset_reduction >= max_anchor_height_offset {
        panic!(
            "max_anchor_height_offset_reduction {} is greater than max_anchor_height_offset from pacaya config {}",
            config.max_anchor_height_offset_reduction, max_anchor_height_offset
        );
    }

    let l1_max_blocks_per_batch = ethereum_l1
        .execution_layer
        .get_config_max_blocks_per_batch();

    if config.max_blocks_per_batch > l1_max_blocks_per_batch {
        panic!(
            "max_blocks_per_batch ({}) exceeds limit from Pacaya config ({})",
            config.max_blocks_per_batch, l1_max_blocks_per_batch
        );
    }

    let max_blocks_per_batch = if config.max_blocks_per_batch == 0 {
        l1_max_blocks_per_batch
    } else {
        config.max_blocks_per_batch
    };

    let chain_monitor = Arc::new(
        chain_monitor::ChainMonitor::new(
            config
                .l1_rpc_urls
                .first()
                .expect("L1 RPC URL is required")
                .clone(),
            config.taiko_geth_rpc_url.clone(),
            pacaya_config.contract_addresses.taiko_inbox.clone(),
            cancel_token.clone(),
        )
        .map_err(|e| anyhow::anyhow!("Failed to create ChainMonitor: {}", e))?,
    );
    chain_monitor
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start ChainMonitor: {}", e))?;

    let handover_window_slots = get_handover_window_slots(&ethereum_l1.execution_layer)
        .await
        .unwrap_or_else(|e| {
            warn!(
                "Failed to get handover window slots: {e}, using default handover window slots: {}",
                pacaya_config.handover_window_slots
            );
            pacaya_config.handover_window_slots
        });

    let node = node::Node::new(
        cancel_token.clone(),
        taiko.clone(),
        ethereum_l1.clone(),
        chain_monitor.clone(),
        transaction_error_receiver,
        metrics.clone(),
        node::config::NodeConfig {
            preconf_heartbeat_ms: config.preconf_heartbeat_ms,
            handover_window_slots,
            handover_start_buffer_ms: pacaya_config.handover_start_buffer_ms,
            l1_height_lag: pacaya_config.l1_height_lag,
            propose_forced_inclusion: pacaya_config.propose_forced_inclusion,
            simulate_not_submitting_at_the_end_of_epoch: pacaya_config
                .simulate_not_submitting_at_the_end_of_epoch,
        },
        node::batch_manager::config::BatchBuilderConfig {
            max_bytes_size_of_batch: config.max_bytes_size_of_batch,
            max_blocks_per_batch,
            l1_slot_duration_sec: config.l1_slot_duration_sec,
            max_time_shift_between_blocks_sec: config.max_time_shift_between_blocks_sec,
            max_anchor_height_offset: max_anchor_height_offset
                - config.max_anchor_height_offset_reduction,
            default_coinbase: ethereum_l1.execution_layer.get_preconfer_alloy_address(),
            preconf_min_txs: config.preconf_min_txs,
            preconf_max_skipped_l2_slots: config.preconf_max_skipped_l2_slots,
        },
        switch_timestamp,
    )
    .await
    .map_err(|e| anyhow::anyhow!("Failed to create Node: {}", e))?;

    node.entrypoint()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start Node: {}", e))?;

    let funds_controller = FundsController::new(
        (&config).into(),
        ethereum_l1.execution_layer.clone(),
        taiko.clone(),
        metrics.clone(),
        cancel_token.clone(),
    );
    funds_controller.run();

    Ok(())
}

async fn get_handover_window_slots(execution_layer: &ExecutionLayer) -> Result<u64, Error> {
    let handover_window_slots = match execution_layer.get_preconf_router_config().await {
        Ok(router_config) => router_config.handOverSlots.try_into().map_err(|e| {
            anyhow::anyhow!("Failed to convert handOverSlots from preconf router config: {e}")
        }),
        Err(e) => return Err(anyhow::anyhow!("Failed to get preconf router config: {e}")),
    };
    if let Ok(handover_window_slots) = handover_window_slots {
        info!(
            "Handover window slots from preconf router config: {}",
            handover_window_slots
        );
    }
    handover_window_slots
}
