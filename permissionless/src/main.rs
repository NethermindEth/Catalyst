use anyhow::Error;
use common::{
    config::ConfigTrait,
    l1::{self as common_l1, ethereum_l1::EthereumL1},
    metrics::Metrics,
    utils as common_utils,
    utils::cancellation_token::CancellationToken,
};
use std::sync::Arc;
use tokio::{
    signal::unix::{SignalKind, signal},
    sync::mpsc,
};
use tracing::{error, info};

mod l1;
mod node;
mod registration;
mod utils;

#[tokio::main]
async fn main() -> Result<(), Error> {
    common_utils::logging::init_logging();

    info!(
        "🚀 Starting Permissionless Node v{}",
        env!("CARGO_PKG_VERSION")
    );

    let config = common::config::Config::read_env_variables()
        .map_err(|e| anyhow::anyhow!("Failed to read configuration: {}", e))?;
    let permissionless_config = crate::utils::config::Config::read_env_variables()
        .map_err(|e| anyhow::anyhow!("Failed to read permissionless configuration: {}", e))?;

    let (transaction_error_sender, transaction_error_receiver) = mpsc::channel(100);
    let metrics = Arc::new(Metrics::new());

    let cancel_token = CancellationToken::new(metrics.clone());
    let panic_cancel_token = cancel_token.clone();
    std::panic::set_hook(Box::new(move |panic_info| {
        error!("Panic occurred: {:?}", panic_info);
        panic_cancel_token.cancel_on_critical_error();
        info!("Cancellation token triggered, initiating shutdown...");
    }));

    let ethereum_l1 = Arc::new(
        EthereumL1::<l1::execution_layer::ExecutionLayer>::new(
            common_l1::config::EthereumL1Config::new(&config).await?,
            l1::config::EthereumL1Config::try_from(permissionless_config.clone())?,
            transaction_error_sender,
            metrics.clone(),
        )
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create EthereumL1: {}", e))?,
    );

    let node = node::Node::new(
        cancel_token.clone(),
        ethereum_l1,
        transaction_error_receiver,
        metrics,
        node::config::NodeConfig {
            preconf_heartbeat_ms: config.preconf_heartbeat_ms,
        },
    )
    .map_err(|e| anyhow::anyhow!("Failed to create Node: {}", e))?;

    node.entrypoint()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start Node: {}", e))?;

    wait_for_the_termination(cancel_token).await;

    Ok(())
}

async fn wait_for_the_termination(cancel_token: CancellationToken) {
    info!("Starting signal handler...");
    let mut sigterm = signal(SignalKind::terminate()).expect("Failed to set up SIGTERM handler");
    tokio::select! {
        _ = sigterm.recv() => {
            info!("Received SIGTERM, shutting down...");
            cancel_token.cancel();
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Received Ctrl+C, shutting down...");
            cancel_token.cancel();
        }
        _ = cancel_token.cancelled() => {
            info!("Shutdown signal received, exiting Catalyst node...");
        }
    }
}
