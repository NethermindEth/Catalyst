use anyhow::Error;
use urc::monitor::config::Config;
use urc::monitor::registry_monitor::RegistryMonitor;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let parse_error = "Failed to parse env filter directive";
    let filter = tracing_subscriber::EnvFilter::from_default_env()
        .add_directive("alloy_transport_http=off".parse().expect(parse_error))
        .add_directive("alloy_rpc_client=off".parse().expect(parse_error))
        .add_directive("reqwest=off".parse().expect(parse_error))
        .add_directive("hyper_util=off".parse().expect(parse_error));

    tracing_subscriber::fmt()
        .with_env_filter(filter) // reads RUST_LOG
        .init();

    tracing::info!("App started");

    let config = Config::new()?;
    let mut registry_monitor = RegistryMonitor::new(config).await?;
    registry_monitor.run_indexing_loop().await?;

    Ok(())
}
