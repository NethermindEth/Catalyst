use anyhow::Error;
use common::{
    fork_info::{Fork, ForkInfo},
    metrics::{self, Metrics},
    shared::execution_layer::ExecutionLayer,
};
use pacaya::create_pacaya_node;
use std::sync::Arc;
use tokio::signal::unix::{SignalKind, signal};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

enum ExecutionStopped {
    CloseApp,
    RecreateNode,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::utils::logging::init_logging();

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

    let config = common::config::Config::read_env_variables();

    let l2_height = ExecutionLayer::new_read_only(&config.taiko_geth_rpc_url)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create L2 execution layer: {}", e))?
        .get_latest_block_id()
        .await?;
    let fork_info = ForkInfo::from_config((&config).into(), l2_height)
        .map_err(|e| anyhow::anyhow!("Failed to get fork info: {}", e))?;

    let cancel_token = CancellationToken::new();

    let metrics = Arc::new(Metrics::new());

    // Set up panic hook to cancel token on panic
    let panic_cancel_token = cancel_token.clone();
    std::panic::set_hook(Box::new(move |panic_info| {
        error!("Panic occurred: {:?}", panic_info);
        panic_cancel_token.cancel();
        info!("Cancellation token triggered, initiating shutdown...");
    }));

    match fork_info.fork {
        Fork::Pacaya => {
            // TODO pacaya::utils::config::Config
            info!(
                "Current fork: Pacaya, switch_timestamp: {:?}",
                fork_info.config.fork_switch_timestamp
            );
            create_pacaya_node(
                config.clone(),
                metrics.clone(),
                cancel_token.clone(),
                fork_info,
            )
            .await?;
        }
        Fork::Shasta => {
            info!("Current fork: Shasta");
            shasta::create_shasta_node(config.clone(), metrics.clone(), cancel_token.clone())
                .await?;
        }
    }

    metrics::server::serve_metrics(metrics.clone(), cancel_token.clone());

    Ok(wait_for_the_termination(cancel_token, config.l1_slot_duration_sec).await)
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
            // prevent rapid recreation of the node in case of initial error
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            ExecutionStopped::RecreateNode
        }
    }
}
