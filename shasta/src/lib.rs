#[allow(dead_code)] // TODO: remove this once we have a used create_shasta_node function
mod node;
#[allow(dead_code)] // TODO: remove this once we have a used create_shasta_node function
mod utils;

mod l1;
mod l2;

use crate::utils::config::ShastaConfig;
use anyhow::Error;
use common::funds_controller::FundsController;
use common::l1::{self as common_l1};
use common::l2::engine::{L2Engine, L2EngineConfig};
use common::{config::Config, config::ConfigTrait, metrics::Metrics};
use l1::execution_layer::ExecutionLayer;
use node::Node;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::info;

pub async fn create_shasta_node(
    config: Config,
    metrics: Arc<Metrics>,
    cancel_token: CancellationToken,
) -> Result<(), Error> {
    info!("Creating Shasta node");

    if !config.disable_bridging {
        return Err(anyhow::anyhow!(
            "Bridging is not implemented. Exiting Shasta node creation."
        ));
    }

    let shasta_config = ShastaConfig::read_env_variables();
    info!("Shasta config: {}", shasta_config);

    let (transaction_error_sender, _transaction_error_receiver) = mpsc::channel(100);
    let ethereum_l1 = common_l1::ethereum_l1::EthereumL1::<ExecutionLayer>::new(
        common_l1::config::EthereumL1Config::new(&config).await?,
        l1::config::EthereumL1Config::try_from(shasta_config.clone())?,
        transaction_error_sender,
        metrics.clone(),
    )
    .await
    .map_err(|e| anyhow::anyhow!("Failed to create EthereumL1: {}", e))?;

    let ethereum_l1 = Arc::new(ethereum_l1);

    let taiko_config = pacaya::l2::config::TaikoConfig::new(&config)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create TaikoConfig: {}", e))?;

    /*  let _el = crate::l2::execution_layer::L2ExecutionLayer::new(taiko_config.clone())
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create L2ExecutionLayer: {}", e))?;
    */
    let l2_engine = L2Engine::new(L2EngineConfig::new(
        &config,
        taiko_config.signer.get_address(),
    )?)
    .map_err(|e| anyhow::anyhow!("Failed to create L2Engine: {}", e))?;

    let taiko = crate::l2::taiko::Taiko::new(
        ethereum_l1.slot_clock.clone(),
        // TODO fetch actual protocol config
        pacaya::l1::protocol_config::ProtocolConfig::default(),
        metrics.clone(),
        taiko_config,
        l2_engine,
    )
    .await?;
    let taiko = Arc::new(taiko);

    let _node = Node::new(cancel_token.clone()).await?;

    let funds_controller = FundsController::new(
        (&config).into(),
        taiko.l2_execution_layer(),
        ethereum_l1.execution_layer.clone(),
        taiko.clone(),
        metrics.clone(),
        cancel_token.clone(),
    );

    funds_controller.run();

    Ok(())
}
