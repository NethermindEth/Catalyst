use crate::signer::Signer;
use anyhow::Error;
use common::{
    fork_info::{Fork, ForkInfo},
    funds_monitor,
    l1::{self as common_l1, el_trait::ELTrait},
    l2,
    metrics::{self, Metrics},
    shared, signer, utils as common_utils,
};
use l1::pacaya::execution_layer::ExecutionLayer;
use std::sync::Arc;
use taiko_mono::{
    indexer::{ShastaEventIndexer, ShastaEventIndexerConfig},
    interface::ShastaProposeInputReader,
};
use tokio::{
    signal::unix::{SignalKind, signal},
    sync::mpsc,
};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

mod chain_monitor;
mod forced_inclusion;
mod l1;
mod node;
mod utils;

enum ExecutionStopped {
    CloseApp,
    RecreateNode,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    common_utils::logging::init_logging();

    info!("🚀 Starting Whitelist Node v{}", env!("CARGO_PKG_VERSION"));

    let mut iteration = 0;
    loop {
        iteration += 1;
        match run_node(iteration).await {
            Ok(ExecutionStopped::CloseApp) => {
                info!("👋 ExecutionStopped::CloseApp , shutting down...");
                break;
            }
            Ok(ExecutionStopped::RecreateNode) => {
                info!("🔄 ExecutionStopped::RecreateNode, recreating node...");
                continue;
            }
            Err(e) => {
                error!("Failed to run node: {}", e);
                return Err(e);
            }
        }
    }

    Ok(())
}

async fn run_node(iteration: u64) -> Result<ExecutionStopped, Error> {
    info!("Running node iteration: {iteration}");

    let fork_info = ForkInfo::from_env()?;

    let config = common_utils::config::Config::<utils::config::Config>::read_env_variables();

    let cancel_token = CancellationToken::new();

    let metrics = Arc::new(Metrics::new());

    // Set up panic hook to cancel token on panic
    let panic_cancel_token = cancel_token.clone();
    std::panic::set_hook(Box::new(move |panic_info| {
        error!("Panic occurred: {:?}", panic_info);
        panic_cancel_token.cancel();
        info!("Cancellation token triggered, initiating shutdown...");
    }));

    let l1_signer = signer::create_signer(
        config.web3signer_l1_url.clone(),
        config.catalyst_node_ecdsa_private_key.clone(),
        config.preconfer_address.clone(),
    )
    .await?;
    let l2_signer = signer::create_signer(
        config.web3signer_l2_url.clone(),
        config.catalyst_node_ecdsa_private_key.clone(),
        config.preconfer_address.clone(),
    )
    .await?;

    match fork_info.fork {
        Fork::Pacaya => {
            info!(
                "Current fork: Pacaya, switch_timestamp: {:?}",
                fork_info.switch_timestamp
            );
            create_pacaya_node(
                config.clone(),
                l1_signer,
                l2_signer,
                metrics.clone(),
                cancel_token.clone(),
                fork_info.switch_timestamp,
            )
            .await?;
        }
        Fork::Shasta => {
            info!("Current fork: Shasta");
            unimplemented!("Shasta fork is not yet implemented");
        }
    }

    metrics::server::serve_metrics(metrics.clone(), cancel_token.clone());

    Ok(wait_for_the_termination(cancel_token, config.l1_slot_duration_sec).await)
}

async fn create_pacaya_node(
    config: common_utils::config::Config<utils::config::Config>,
    l1_signer: Arc<Signer>,
    l2_signer: Arc<Signer>,
    metrics: Arc<Metrics>,
    cancel_token: CancellationToken,
    switch_timestamp: Option<u64>,
) -> Result<(), Error> {
    let (transaction_error_sender, transaction_error_receiver) = mpsc::channel(100);
    let ethereum_l1 = common_l1::ethereum_l1::EthereumL1::<ExecutionLayer>::new(
        common_l1::config::EthereumL1Config::new(&config, l1_signer),
        l1::pacaya::config::EthereumL1Config::try_from(config.specific_config.clone())?,
        transaction_error_sender,
        metrics.clone(),
    )
    .await
    .map_err(|e| anyhow::anyhow!("Failed to create EthereumL1: {}", e))?;

    let ethereum_l1 = Arc::new(ethereum_l1);

    let jwt_secret_bytes =
        common_utils::file_operations::read_jwt_secret(&config.jwt_secret_file_path)?;
    let taiko = Arc::new(
        l2::taiko::Taiko::new(
            ethereum_l1.clone(),
            metrics.clone(),
            l2::config::TaikoConfig::new(
                config.taiko_geth_rpc_url.clone(),
                config.taiko_geth_auth_rpc_url.clone(),
                config.taiko_driver_url.clone(),
                jwt_secret_bytes,
                config.taiko_anchor_address.clone(),
                config.taiko_bridge_address.clone(),
                config.max_bytes_per_tx_list,
                config.min_bytes_per_tx_list,
                config.throttling_factor,
                config.rpc_l2_execution_layer_timeout,
                config.rpc_driver_preconf_timeout,
                config.rpc_driver_status_timeout,
                l2_signer,
            )?,
        )
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create Taiko: {}", e))?,
    );

    let max_anchor_height_offset = ethereum_l1
        .execution_layer
        .common()
        .get_config_max_anchor_height_offset();
    if config.max_anchor_height_offset_reduction >= max_anchor_height_offset {
        panic!(
            "max_anchor_height_offset_reduction {} is greater than max_anchor_height_offset from pacaya config {}",
            config.max_anchor_height_offset_reduction, max_anchor_height_offset
        );
    }

    let l1_max_blocks_per_batch = ethereum_l1
        .execution_layer
        .common()
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
            config
                .specific_config
                .contract_addresses
                .taiko_inbox
                .clone(),
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
                config.specific_config.handover_window_slots
            );
            config.specific_config.handover_window_slots
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
            handover_start_buffer_ms: config.specific_config.handover_start_buffer_ms,
            l1_height_lag: config.specific_config.l1_height_lag,
            propose_forced_inclusion: config.specific_config.propose_forced_inclusion,
            simulate_not_submitting_at_the_end_of_epoch: config
                .specific_config
                .simulate_not_submitting_at_the_end_of_epoch,
        },
        node::batch_manager::config::BatchBuilderConfig {
            max_bytes_size_of_batch: config.max_bytes_size_of_batch,
            max_blocks_per_batch,
            l1_slot_duration_sec: config.l1_slot_duration_sec,
            max_time_shift_between_blocks_sec: config.max_time_shift_between_blocks_sec,
            max_anchor_height_offset: max_anchor_height_offset
                - config.max_anchor_height_offset_reduction,
            default_coinbase: ethereum_l1
                .execution_layer
                .common()
                .get_preconfer_alloy_address(),
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

    let funds_monitor = funds_monitor::FundsMonitor::new(
        ethereum_l1.clone(),
        taiko.clone(),
        metrics.clone(),
        config.threshold_eth,
        config.threshold_taiko,
        config.amount_to_bridge_from_l2_to_l1,
        config.disable_bridging,
        cancel_token.clone(),
        config.bridge_relayer_fee,
        config.bridge_transaction_fee,
    );
    funds_monitor.run();

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

async fn wait_for_the_termination(
    cancel_token: CancellationToken,
    shutdown_delay_secs: u64,
) -> ExecutionStopped {
    info!("Starting signal handler...");
    let mut sigterm = signal(SignalKind::terminate()).expect("Failed to set up SIGTERM handler");
    tokio::select! {
        _ = sigterm.recv() => {
            info!("Received SIGTERM, shutting down...");
            cancel_token.cancel();
            // Give tasks a little time to finish
            info!("Waiting for {}s", shutdown_delay_secs);
            tokio::time::sleep(tokio::time::Duration::from_secs(shutdown_delay_secs)).await;
            ExecutionStopped::CloseApp
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Received Ctrl+C, shutting down...");
            cancel_token.cancel();
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            ExecutionStopped::CloseApp
        }
        _ = cancel_token.cancelled() => {
            info!("Shutdown signal received, exiting Catalyst node...");
            ExecutionStopped::RecreateNode
        }
    }
}
