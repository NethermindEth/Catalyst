use anyhow::Error;
use common::{
    chain_monitor, ethereum_l1, funds_monitor, metrics, shared, taiko, utils as common_utils,
};
use metrics::Metrics;
use shared::signer::Signer;
use std::{sync::Arc, time::Duration};
use tokio::{
    signal::unix::{SignalKind, signal},
    sync::mpsc,
};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

mod forced_inclusion;
mod l1;
mod node;
mod utils;

#[cfg(feature = "test-gas")]
use clap::Parser;
#[cfg(feature = "test-gas")]
use common::test_gas::test_gas_params;

#[cfg(feature = "test-gas")]
#[derive(Parser, Debug)]
struct Args {
    #[arg(long = "test-gas", value_name = "BLOCK_COUNT")]
    test_gas: Option<u32>,
}

const SIGNER_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::main]
async fn main() -> Result<(), Error> {
    common_utils::logging::init_logging();

    info!("🚀 Starting Whitelist Node v{}", env!("CARGO_PKG_VERSION"));

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

    let (transaction_error_sender, transaction_error_receiver) = mpsc::channel(100);

    let l1_signer = create_signer(
        config.web3signer_l1_url.clone(),
        config.catalyst_node_ecdsa_private_key.clone(),
        config.preconfer_address.clone(),
    )
    .await?;
    let l2_signer = create_signer(
        config.web3signer_l2_url.clone(),
        config.catalyst_node_ecdsa_private_key.clone(),
        config.preconfer_address.clone(),
    )
    .await?;

    let ethereum_l1 = ethereum_l1::EthereumL1::<l1::execution_layer::ExecutionLayer>::new(
        ethereum_l1::config::EthereumL1Config {
            execution_rpc_urls: config.l1_rpc_urls.clone(),
            contract_addresses: config
                .specific_config
                .contract_addresses
                .clone()
                .try_into()?,
            consensus_rpc_url: config.l1_beacon_url,
            slot_duration_sec: config.l1_slot_duration_sec,
            slots_per_epoch: config.l1_slots_per_epoch,
            preconf_heartbeat_ms: config.preconf_heartbeat_ms,
            min_priority_fee_per_gas_wei: config.min_priority_fee_per_gas_wei,
            tx_fees_increase_percentage: config.tx_fees_increase_percentage,
            max_attempts_to_send_tx: config.max_attempts_to_send_tx,
            max_attempts_to_wait_tx: config.max_attempts_to_wait_tx,
            delay_between_tx_attempts_sec: config.delay_between_tx_attempts_sec,
            signer: l1_signer,
            preconfer_address: config.preconfer_address.clone().map(|s| {
                s.parse()
                    .expect("Preconfer address is not a valid Ethereum address")
            }),
            extra_gas_percentage: config.extra_gas_percentage,
        },
        l1::execution_layer::EthereumL1Config {
            contract_addresses: config
                .specific_config
                .contract_addresses
                .clone()
                .try_into()?,
        },
        transaction_error_sender,
        metrics.clone(),
    )
    .await
    .map_err(|e| anyhow::anyhow!("Failed to create EthereumL1: {}", e))?;

    let ethereum_l1 = Arc::new(ethereum_l1);

    #[cfg(feature = "test-gas")]
    let args = Args::parse();
    #[cfg(feature = "test-gas")]
    if let Some(gas) = args.test_gas {
        info!("Test gas block count: {}", gas);
        test_gas_params(
            ethereum_l1.clone(),
            gas,
            config.l1_height_lag,
            config.max_bytes_size_of_batch,
            transaction_error_receiver,
        )
        .await?;
        return Ok(());
    } else {
        tracing::error!("No test gas block count provided.");
    }

    let jwt_secret_bytes =
        common_utils::file_operations::read_jwt_secret(&config.jwt_secret_file_path)?;
    let taiko = Arc::new(
        taiko::Taiko::new(
            ethereum_l1.clone(),
            metrics.clone(),
            taiko::config::TaikoConfig::new(
                config.taiko_geth_rpc_url.clone(),
                config.taiko_geth_auth_rpc_url,
                config.taiko_driver_url,
                jwt_secret_bytes,
                config.taiko_anchor_address,
                config.taiko_bridge_address,
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
            config.taiko_geth_rpc_url,
            config.specific_config.contract_addresses.taiko_inbox,
            cancel_token.clone(),
        )
        .map_err(|e| anyhow::anyhow!("Failed to create ChainMonitor: {}", e))?,
    );
    chain_monitor
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start ChainMonitor: {}", e))?;

    let node = node::Node::new(
        cancel_token.clone(),
        taiko.clone(),
        ethereum_l1.clone(),
        chain_monitor.clone(),
        transaction_error_receiver,
        metrics.clone(),
        node::NodeConfig {
            preconf_heartbeat_ms: config.preconf_heartbeat_ms,
            handover_window_slots: config.handover_window_slots,
            handover_start_buffer_ms: config.handover_start_buffer_ms,
            l1_height_lag: config.l1_height_lag,
            propose_forced_inclusion: config.propose_forced_inclusion,
            simulate_not_submitting_at_the_end_of_epoch: config
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

    metrics::server::serve_metrics(metrics.clone(), cancel_token.clone());

    wait_for_the_termination(cancel_token, config.l1_slot_duration_sec).await;

    Ok(())
}

async fn create_signer(
    web3signer_url: Option<String>,
    catalyst_node_ecdsa_private_key: Option<String>,
    preconfer_address: Option<String>,
) -> Result<Arc<Signer>, Error> {
    Ok(Arc::new(if let Some(web3signer_url) = web3signer_url {
        Signer::Web3signer(Arc::new(
            shared::web3signer::Web3Signer::new(
                &web3signer_url,
                SIGNER_TIMEOUT,
                preconfer_address
                    .as_ref()
                    .expect("preconfer address is required for web3signer usage"),
            )
            .await?,
        ))
    } else if let Some(catalyst_node_ecdsa_private_key) = catalyst_node_ecdsa_private_key {
        Signer::PrivateKey(catalyst_node_ecdsa_private_key)
    } else {
        panic!("No signer provided");
    }))
}

async fn wait_for_the_termination(cancel_token: CancellationToken, shutdown_delay_secs: u64) {
    info!("Starting signal handler...");
    let mut sigterm = signal(SignalKind::terminate()).expect("Failed to set up SIGTERM handler");
    tokio::select! {
        _ = sigterm.recv() => {
            info!("Received SIGTERM, shutting down...");
            cancel_token.cancel();
            // Give tasks a little time to finish
            info!("Waiting for {}s", shutdown_delay_secs);
            tokio::time::sleep(tokio::time::Duration::from_secs(shutdown_delay_secs)).await;
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Received Ctrl+C, shutting down...");
            cancel_token.cancel();
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }
        _ = cancel_token.cancelled() => {
            info!("Shutdown signal received, exiting Catalyst node...");
        }
    }
}
