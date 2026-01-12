#![allow(dead_code)]

use anyhow::Error;
use clap::Parser;
use common::config::ConfigTrait;
use common::{l1 as common_l1, utils as common_utils};
use pacaya::utils::config::PacayaConfig;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::info;

// Module declarations
mod test_gas_params;

// Imports
use pacaya::l1::config::EthereumL1Config;
use pacaya::l1::execution_layer::ExecutionLayer;

#[derive(Parser, Debug)]
#[command(name = "test-gas")]
#[command(about = "Test gas parameters for batch submission")]
struct Args {
    #[arg(long = "test-gas", value_name = "BLOCK_COUNT")]
    test_gas: u32,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    common_utils::logging::init_logging();

    info!("🚀 Starting Test Gas Tool v{}", env!("CARGO_PKG_VERSION"));

    let args = Args::parse();

    let config = common::config::Config::read_env_variables()
        .map_err(|e| anyhow::anyhow!("Failed to read configuration: {}", e))?;

    let pacaya_config = PacayaConfig::read_env_variables()
        .map_err(|e| anyhow::anyhow!("Failed to read Pacaya configuration: {}", e))?;

    let (transaction_error_sender, transaction_error_receiver) = mpsc::channel(100);

    let ethereum_l1 = common_l1::ethereum_l1::EthereumL1::<ExecutionLayer>::new(
        common_l1::config::EthereumL1Config::new(&config).await?,
        EthereumL1Config::try_from(pacaya_config.clone())?,
        transaction_error_sender,
        Arc::new(common::metrics::Metrics::new()),
    )
    .await
    .map_err(|e| anyhow::anyhow!("Failed to create EthereumL1: {}", e))?;

    let ethereum_l1 = Arc::new(ethereum_l1);

    info!("Test gas block count: {}", args.test_gas);
    test_gas_params::test_gas_params(
        ethereum_l1,
        args.test_gas,
        pacaya_config.l1_height_lag,
        config.max_bytes_size_of_batch,
        transaction_error_receiver,
    )
    .await?;

    Ok(())
}
