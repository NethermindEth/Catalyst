use anyhow::Error;
use common::{
    fork_info::{Fork, ForkInfo},
    metrics::{self, Metrics},
    utils::cancellation_token::CancellationToken,
};
use pacaya::create_pacaya_node;
use std::sync::Arc;
use tokio::signal::unix::{SignalKind, signal};
use tracing::{error, info};

// Initialize rustls crypto provider before any TLS operations
fn init_rustls() {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Failed to install default rustls crypto provider");
}

enum ExecutionStopped {
    CloseApp,
    RecreateNode,
}

const WAIT_BEFORE_RECREATING_NODE_SECS: u64 = 5;

#[tokio::main]
async fn main() -> Result<(), Error> {
    init_rustls();

    common::utils::logging::init_logging();

    info!("🚀 Starting Catalyst Node v{}", env!("CARGO_PKG_VERSION"));

    let mut iteration = 0;
    let metrics = Arc::new(Metrics::new());
    loop {
        iteration += 1;
        match run_node(iteration, metrics.clone()).await {
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
                metrics.inc_critical_errors();
                info!(
                    "Waiting {WAIT_BEFORE_RECREATING_NODE_SECS} second before recreating node..."
                );
                tokio::time::sleep(tokio::time::Duration::from_secs(
                    WAIT_BEFORE_RECREATING_NODE_SECS,
                ))
                .await;
                continue;
            }
        }
    }

    Ok(())
}

async fn run_node(iteration: u64, metrics: Arc<Metrics>) -> Result<ExecutionStopped, Error> {
    info!("Running node iteration: {iteration}");

    let config = common::config::Config::read_env_variables()
        .map_err(|e| anyhow::anyhow!("Failed to read configuration: {}", e))?;

    let fork_info = ForkInfo::from_config((&config).into())
        .map_err(|e| anyhow::anyhow!("Failed to get fork info: {}", e))?;

    let cancel_token = CancellationToken::new(metrics.clone());

    // Set up panic hook to cancel token on panic
    let panic_cancel_token = cancel_token.clone();
    std::panic::set_hook(Box::new(move |panic_info| {
        error!("Panic occurred: {:?}", panic_info);
        panic_cancel_token.cancel_on_critical_error();
        info!("Cancellation token triggered, initiating shutdown...");
    }));

    match fork_info.fork {
        Fork::Pacaya => {
            // TODO pacaya::utils::config::Config
            let next_fork_timestamp = fork_info.config.fork_switch_timestamps.get(1);
            info!(
                "Current fork: PACAYA 🌋, next fork timestamp: {:?}",
                next_fork_timestamp
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
            info!("Current fork: SHASTA 🌋");
            shasta::create_shasta_node(
                config.clone(),
                metrics.clone(),
                cancel_token.clone(),
                fork_info,
            )
            .await?;
        }
        Fork::Permissionless => {
            info!("Current fork: PERMISSIONLESS 🌋");
            permissionless::create_permissionless_node(
                config.clone(),
                metrics.clone(),
                cancel_token.clone(),
                fork_info,
            )
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
            tokio::time::sleep(tokio::time::Duration::from_secs(WAIT_BEFORE_RECREATING_NODE_SECS)).await;
            ExecutionStopped::RecreateNode
        }
    }
}
