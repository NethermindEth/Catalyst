#[allow(dead_code)] // TODO: remove this once we have a used create_shasta_node function
mod node;
#[allow(dead_code)] // TODO: remove this once we have a used create_shasta_node function
mod utils;

mod l1;
mod l2;

use crate::{l1::event_indexer::EventIndexer, utils::config::ShastaConfig};
use anyhow::Error;
use common::{l1::traits::PreconferProvider, metrics, shared};

use common::funds_controller::FundsController;
use common::l1::{self as common_l1};
use common::l2::engine::{L2Engine, L2EngineConfig};
use common::{config::Config, config::ConfigTrait};
use l1::execution_layer::ExecutionLayer;
use node::Node;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::info;

pub async fn create_shasta_node(
    config: Config,
    metrics: Arc<metrics::Metrics>,
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

    let event_indexer = Arc::new(
        EventIndexer::new(
            config
                .l1_rpc_urls
                .first()
                .expect("L1 RPC URL is required")
                .clone(),
            shasta_config.shasta_inbox.clone(),
        )
        .await?,
    );

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

    // TODO fix
    let node_config = pacaya::node::config::NodeConfig {
        preconf_heartbeat_ms: config.preconf_heartbeat_ms,
        handover_window_slots: 4,
        handover_start_buffer_ms: 500,
        l1_height_lag: 5,
        propose_forced_inclusion: false,
        simulate_not_submitting_at_the_end_of_epoch: false,
    };

    let max_blocks_per_batch = if config.max_blocks_per_batch == 0 {
        5 // TODO fetch actual max from protocol config
    } else {
        config.max_blocks_per_batch
    };

    let max_anchor_height_offset = 64; // TODO fetch actual max from protocol config

    let batch_builder_config = node::proposal_manager::config::BatchBuilderConfig {
        max_bytes_size_of_batch: config.max_bytes_size_of_batch,
        max_blocks_per_batch,
        l1_slot_duration_sec: config.l1_slot_duration_sec,
        max_time_shift_between_blocks_sec: config.max_time_shift_between_blocks_sec,
        max_anchor_height_offset: max_anchor_height_offset
            - config.max_anchor_height_offset_reduction,
        default_coinbase: ethereum_l1.execution_layer.get_preconfer_alloy_address(),
        preconf_min_txs: config.preconf_min_txs,
        preconf_max_skipped_l2_slots: config.preconf_max_skipped_l2_slots,
    };

    let node = Node::new(
        node_config,
        cancel_token.clone(),
        ethereum_l1.clone(),
        taiko.clone(),
        event_indexer,
        metrics.clone(),
        batch_builder_config,
    )
    .await
    .map_err(|e| anyhow::anyhow!("Failed to create Node: {}", e))?;

    node.entrypoint()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start Node: {}", e))?;

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
