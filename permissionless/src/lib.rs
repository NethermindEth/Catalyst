mod l1;
mod l2;
mod node;
mod registration;
mod utils;

use crate::node::config::NodeConfig;
use crate::utils::config::Config as PermissionlessConfig;
use anyhow::Error;
use common::{
    batch_builder::BatchBuilderConfig,
    config::Config,
    config::ConfigTrait,
    fork_info::ForkInfo,
    l1::{
        self as common_l1,
        ethereum_l1::EthereumL1,
        traits::{ELTrait, PreconferProvider},
    },
    l2::engine::{L2Engine, L2EngineConfig},
    metrics::Metrics,
    utils::cancellation_token::CancellationToken,
};
use shasta::l1::execution_layer::ExecutionLayer as ShastaExecutionLayer;
use shasta::{
    ProposalManager, l1::config::EthereumL1Config as ShastaEthereumL1Config, l2::taiko::Taiko,
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
    let shasta_l1_config = ShastaEthereumL1Config {
        shasta_inbox: permissionless_config.shasta_inbox,
    };
    let ethereum_l1 = Arc::new(
        EthereumL1::<ShastaExecutionLayer>::new(
            common_l1::config::EthereumL1Config::new(&config).await?,
            shasta_l1_config,
            transaction_error_sender,
            metrics.clone(),
        )
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create EthereumL1: {}", e))?,
    );
    let preconfer_address = ethereum_l1.execution_layer.common().preconfer_address();

    let taiko_config = pacaya::l2::config::TaikoConfig::new(&config)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create TaikoConfig: {}", e))?;

    let l2_engine = L2Engine::new(L2EngineConfig::new(
        &config,
        taiko_config.signer.get_address(),
    )?)
    .map_err(|e| anyhow::anyhow!("Failed to create L2Engine: {}", e))?;
    let protocol_config = ethereum_l1.execution_layer.fetch_protocol_config().await?;

    let taiko = Taiko::new(
        ethereum_l1.slot_clock.clone(),
        protocol_config.clone(),
        metrics.clone(),
        taiko_config,
        l2_engine,
    )
    .await?;
    let taiko = Arc::new(taiko);

    if permissionless_config.max_blocks_to_reanchor
        >= taiko.get_protocol_config().get_timestamp_max_offset()
    {
        return Err(anyhow::anyhow!(
            "MAX_BLOCKS_TO_REANCHOR ({}) must be less than TIMESTAMP_MAX_OFFSET ({})",
            permissionless_config.max_blocks_to_reanchor,
            taiko.get_protocol_config().get_timestamp_max_offset()
        ));
    }

    let max_blocks_per_batch = if config.max_blocks_per_batch == 0 {
        taiko_protocol::shasta::constants::DERIVATION_SOURCE_MAX_BLOCKS.try_into()?
    } else {
        config.max_blocks_per_batch
    };

    let max_anchor_height_offset = taiko.get_protocol_config().get_max_anchor_height_offset();

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

    let proposal_manager = ProposalManager::new(
        permissionless_config.l1_height_lag,
        batch_builder_config,
        ethereum_l1.clone(),
        taiko.clone(),
        metrics.clone(),
        cancel_token.clone(),
        permissionless_config.max_blocks_to_reanchor,
        permissionless_config.propose_forced_inclusion,
    )
    .await
    .map_err(|e| anyhow::anyhow!("Failed to create ProposalManager: {}", e))?;

    let preconfirmation_driver = Arc::new(
        l2::preconfirmation_driver::PreconfirmationDriver::new_with_timeout(
            &permissionless_config.preconfirmation_driver_url,
            permissionless_config.preconfirmation_driver_timeout,
        )
        .map_err(|e| anyhow::anyhow!("Failed to create PreconfirmationDriver: {}", e))?,
    );
    let operator = crate::node::operator::Operator::new(preconfirmation_driver, preconfer_address);

    let node = node::Node::new(
        cancel_token.clone(),
        ethereum_l1,
        transaction_error_receiver,
        metrics,
        NodeConfig {
            preconf_heartbeat_ms: config.preconf_heartbeat_ms,
        },
        operator,
        proposal_manager,
        taiko,
    )
    .map_err(|e| anyhow::anyhow!("Failed to create Node: {}", e))?;

    node.entrypoint()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start Node: {}", e))?;

    Ok(())
}
