use anyhow::Error;
use common::{config::Config, metrics::Metrics, utils::cancellation_token::CancellationToken};
use permissionless::create_permissionless_node;
use std::sync::Arc;
use tokio::signal::unix::{SignalKind, signal};
use tracing::{error, info};

// Initialize rustls crypto provider before any TLS operations
fn init_rustls() {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Failed to install default rustls crypto provider");
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    init_rustls();

    common::utils::logging::init_logging();

    info!(
        "🚀 Starting Permissionless Node v{}",
        env!("CARGO_PKG_VERSION")
    );

    let config = Config::read_env_variables()
        .map_err(|e| anyhow::anyhow!("Failed to read configuration: {}", e))?;

    let fork_info = common::fork_info::ForkInfo::from_config((&config).into())
        .map_err(|e| anyhow::anyhow!("Failed to get fork info: {}", e))?;

    let metrics = Arc::new(Metrics::new());
    let cancel_token = CancellationToken::new(metrics.clone());

    // Set up panic hook to cancel token on panic
    let panic_cancel_token = cancel_token.clone();
    std::panic::set_hook(Box::new(move |panic_info| {
        error!("Panic occurred: {:?}", panic_info);
        panic_cancel_token.cancel_on_critical_error();
        info!("Cancellation token triggered, initiating shutdown...");
    }));

    create_permissionless_node(config, metrics, cancel_token.clone(), fork_info).await?;

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
