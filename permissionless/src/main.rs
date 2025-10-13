use anyhow::Error;
use common::{
    l1 as common_l1, l1::ethereum_l1::EthereumL1, metrics::Metrics, signer, utils as common_utils,
};
use std::sync::Arc;
use tokio::{
    signal::unix::{SignalKind, signal},
    sync::mpsc,
};
use tokio_util::sync::CancellationToken;
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

    let config = common_utils::config::Config::read_env_variables();
    let (transaction_error_sender, transaction_error_receiver) = mpsc::channel(100);
    let metrics = Arc::new(Metrics::new());

    let cancel_token = CancellationToken::new();
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

    let ethereum_l1 = Arc::new(
        EthereumL1::<l1::execution_layer::ExecutionLayer>::new(
            common_l1::config::EthereumL1Config::new(&config, l1_signer),
            l1::config::EthereumL1Config::try_from(config.specific_config.clone())?,
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
        node::config::NodeConfig::from(config.clone()),
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
