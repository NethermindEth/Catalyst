mod l1;
mod node;
mod registration;
mod utils;

use crate::l1::execution_layer::ExecutionLayer;
use crate::node::config::NodeConfig;
use crate::utils::config::Config as PermissionlessConfig;
use anyhow::Error;
use common::{
    config::Config,
    config::ConfigTrait,
    fork_info::ForkInfo,
    l1::{self as common_l1, ethereum_l1::EthereumL1},
    metrics::Metrics,
    utils::cancellation_token::CancellationToken,
};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::info;

pub async fn create_permissionless_node(
    config: Config,
    metrics: Arc<Metrics>,
    cancel_token: CancellationToken,
    _fork_info: ForkInfo,
) -> Result<(), Error> {
    info!("Creating Permissionless node");

    let permissionless_config = PermissionlessConfig::read_env_variables()
        .map_err(|e| anyhow::anyhow!("Failed to read permissionless configuration: {}", e))?;
    info!("Permissionless config: {}", permissionless_config);

    let (transaction_error_sender, transaction_error_receiver) = mpsc::channel(100);
    let ethereum_l1 = Arc::new(
        EthereumL1::<ExecutionLayer>::new(
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
        NodeConfig {
            preconf_heartbeat_ms: config.preconf_heartbeat_ms,
        },
    )
    .map_err(|e| anyhow::anyhow!("Failed to create Node: {}", e))?;

    node.entrypoint()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start Node: {}", e))?;

    Ok(())
}
