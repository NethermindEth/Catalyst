mod chain_monitor;
mod l1;
mod l2;
mod node;
pub mod raiko;
mod shared_abi;
mod utils;

use crate::utils::config::RealtimeConfig;
use anyhow::Error;
use common::{
    batch_builder::BatchBuilderConfig,
    config::Config,
    config::ConfigTrait,
    fork_info::ForkInfo,
    l1::{self as common_l1, traits::PreconferProvider},
    l2::engine::{L2Engine, L2EngineConfig},
    metrics,
    utils::cancellation_token::CancellationToken,
};
use l1::execution_layer::ExecutionLayer;
use node::Node;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::info;

pub async fn create_realtime_node(
    config: Config,
    metrics: Arc<metrics::Metrics>,
    cancel_token: CancellationToken,
    fork_info: ForkInfo,
) -> Result<(), Error> {
    info!("Creating RealTime node");

    let realtime_config = RealtimeConfig::read_env_variables()
        .map_err(|e| anyhow::anyhow!("Failed to read RealTime configuration: {}", e))?;
    info!("RealTime config: {}", realtime_config);

    let (transaction_error_sender, transaction_error_receiver) = mpsc::channel(100);
    let ethereum_l1 = common_l1::ethereum_l1::EthereumL1::<ExecutionLayer>::new(
        common_l1::config::EthereumL1Config::new(&config).await?,
        l1::config::EthereumL1Config::try_from(realtime_config.clone())?,
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
        config.taiko_bridge_address,
        realtime_config.l2_signal_service,
    )
    .await?;
    let taiko = Arc::new(taiko);

    let node_config = pacaya::node::config::NodeConfig {
        preconf_heartbeat_ms: config.preconf_heartbeat_ms,
        handover_window_slots: 8,
        handover_start_buffer_ms: 500,
        l1_height_lag: 8,
        propose_forced_inclusion: false,
        simulate_not_submitting_at_the_end_of_epoch: false,
        watchdog_max_counter: config.watchdog_max_counter,
    };

    let max_blocks_per_batch = if config.max_blocks_per_batch == 0 {
        taiko_protocol::shasta::constants::DERIVATION_SOURCE_MAX_BLOCKS.try_into()?
    } else {
        config.max_blocks_per_batch
    };

    // Use 256-block limit for anchor offset
    let max_anchor_height_offset = 256u64;

    let batch_builder_config = BatchBuilderConfig {
        max_bytes_size_of_batch: config.max_bytes_size_of_batch,
        max_blocks_per_batch,
        l1_slot_duration_sec: config.l1_slot_duration_sec,
        max_time_shift_between_blocks_sec: config.max_time_shift_between_blocks_sec,
        max_anchor_height_offset: max_anchor_height_offset
            - config.max_anchor_height_offset_reduction,
        default_coinbase: ethereum_l1.execution_layer.get_preconfer_address(),
        preconf_min_txs: config.preconf_min_txs,
        preconf_max_skipped_l2_slots: config.preconf_max_skipped_l2_slots,
        proposal_max_time_sec: config.proposal_max_time_sec,
    };

    // Initialize chain monitor for ProposedAndProved events
    let chain_monitor = Arc::new(
        chain_monitor::RealtimeChainMonitor::new(
            config
                .l1_rpc_urls
                .first()
                .ok_or_else(|| anyhow::anyhow!("L1 RPC URL is required"))?
                .clone(),
            config.taiko_geth_rpc_url.clone(),
            realtime_config.realtime_inbox,
            cancel_token.clone(),
            "ProposedAndProved",
            chain_monitor::print_proposed_and_proved_info,
            metrics.clone(),
        )
        .map_err(|e| anyhow::anyhow!("Failed to create RealtimeChainMonitor: {}", e))?,
    );
    chain_monitor
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start RealtimeChainMonitor: {}", e))?;

    // Read the last finalized block hash from L1
    let last_finalized_block_hash = ethereum_l1
        .execution_layer
        .get_last_finalized_block_hash()
        .await?;
    info!(
        "Initial lastFinalizedBlockHash: {}",
        last_finalized_block_hash
    );

    let preconf_only = realtime_config.preconf_only;
    let proof_request_bypass = realtime_config.proof_request_bypass;
    let bridge_rpc_addr = realtime_config.bridge_rpc_addr.clone();
    let raiko_client = raiko::RaikoClient::new(&realtime_config);

    let l1_chain_id = {
        use common::l1::traits::ELTrait;
        ethereum_l1.execution_layer.common().chain_id()
    };
    let l2_chain_id = taiko.l2_execution_layer().chain_id;

    let node = Node::new(
        node_config,
        cancel_token.clone(),
        ethereum_l1.clone(),
        taiko.clone(),
        metrics.clone(),
        batch_builder_config,
        transaction_error_receiver,
        fork_info,
        last_finalized_block_hash,
        raiko_client,
        protocol_config.basefee_sharing_pctg,
        preconf_only,
        proof_request_bypass,
        bridge_rpc_addr,
        l1_chain_id,
        l2_chain_id,
    )
    .await
    .map_err(|e| anyhow::anyhow!("Failed to create Node: {}", e))?;

    node.entrypoint()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start Node: {}", e))?;

    Ok(())
}
