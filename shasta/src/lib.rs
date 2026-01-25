mod chain_monitor;
mod forced_inclusion;
mod l1;
mod l2;
mod node;
mod utils;

use crate::utils::config::ShastaConfig;
use anyhow::Error;
use common::{
    batch_builder::BatchBuilderConfig,
    config::Config,
    config::ConfigTrait,
    fork_info::ForkInfo,
    funds_controller::FundsController,
    l1::{self as common_l1, traits::PreconferProvider},
    l2::engine::{L2Engine, L2EngineConfig},
    metrics, shared,
    utils::cancellation_token::CancellationToken,
};
use l1::execution_layer::ExecutionLayer;
use node::Node;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::info;

pub async fn create_shasta_node(
    config: Config,
    metrics: Arc<metrics::Metrics>,
    cancel_token: CancellationToken,
    fork_info: ForkInfo,
) -> Result<(), Error> {
    info!("Creating Shasta node");

    if !config.disable_bridging {
        return Err(anyhow::anyhow!(
            "Bridging is not implemented. Exiting Shasta node creation."
        ));
    }

    let shasta_config = ShastaConfig::read_env_variables()
        .map_err(|e| anyhow::anyhow!("Failed to read Shasta configuration: {}", e))?;
    info!("Shasta config: {}", shasta_config);

    let (transaction_error_sender, transaction_error_receiver) = mpsc::channel(100);
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
    let protocol_config = ethereum_l1.execution_layer.fetch_protocol_config().await?;

    let taiko = crate::l2::taiko::Taiko::new(
        ethereum_l1.slot_clock.clone(),
        protocol_config.clone(),
        metrics.clone(),
        taiko_config,
        l2_engine,
    )
    .await?;
    let taiko = Arc::new(taiko);

    // TODO fix
    let node_config = pacaya::node::config::NodeConfig {
        preconf_heartbeat_ms: config.preconf_heartbeat_ms,
        handover_window_slots: 8,
        handover_start_buffer_ms: 500,
        l1_height_lag: 8,
        propose_forced_inclusion: true,
        simulate_not_submitting_at_the_end_of_epoch: false,
    };

    let max_blocks_per_batch = if config.max_blocks_per_batch == 0 {
        taiko_protocol::shasta::constants::PROPOSAL_MAX_BLOCKS.try_into()?
    } else {
        config.max_blocks_per_batch
    };

    let max_anchor_height_offset = taiko_protocol::shasta::constants::MAX_ANCHOR_OFFSET;

    let batch_builder_config = BatchBuilderConfig {
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

    let chain_monitor = Arc::new(
        chain_monitor::ShastaChainMonitor::new(
            config
                .l1_rpc_urls
                .first()
                .expect("L1 RPC URL is required")
                .clone(),
            config.taiko_geth_rpc_url.clone(),
            shasta_config.shasta_inbox,
            cancel_token.clone(),
            "Proposed",
            chain_monitor::print_proposed_info,
        )
        .map_err(|e| anyhow::anyhow!("Failed to create ShastaChainMonitor: {}", e))?,
    );
    chain_monitor
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start ShastaChainMonitor: {}", e))?;

    let node = Node::new(
        node_config,
        cancel_token.clone(),
        ethereum_l1.clone(),
        taiko.clone(),
        metrics.clone(),
        batch_builder_config,
        transaction_error_receiver,
        fork_info,
        chain_monitor.clone(),
    )
    .await
    .map_err(|e| anyhow::anyhow!("Failed to create Node: {}", e))?;

    node.entrypoint()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start Node: {}", e))?;

    let funds_controller = FundsController::new(
        (&config).into(),
        ethereum_l1.execution_layer.clone(),
        taiko.clone(),
        metrics.clone(),
        cancel_token.clone(),
    );
    funds_controller.run();

    let whitelist_monitor = pacaya::chain_monitor::WhitelistMonitor::new(
        ethereum_l1.execution_layer.clone(),
        cancel_token.clone(),
        metrics.clone(),
        config.whitelist_monitor_interval_sec,
    );
    whitelist_monitor.run();

    Ok(())
}
