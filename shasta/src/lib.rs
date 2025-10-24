mod event_indexer;
#[allow(dead_code)] // TODO: remove this once we have a used create_shasta_node function
mod node;
#[allow(dead_code)] // TODO: remove this once we have a used create_shasta_node function
mod utils;

mod l1;
mod l2;

use crate::{event_indexer::EventIndexer, utils::config::ShastaConfig};
use anyhow::Error;
use common::l1::traits::ELTrait;
use common::l1::{self as common_l1};
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

    let shasta_config = ShastaConfig::read_env_variables();
    let event_indexer = EventIndexer::new(
        config
            .l1_rpc_urls
            .first()
            .expect("L1 RPC URL is required")
            .clone(),
        shasta_config.contract_addresses.shasta_inbox.clone(),
        config
            .fork_switch_l2_height
            .ok_or_else(|| anyhow::anyhow!("Fork switch L2 height is required"))?,
    )
    .await?;

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

    let execution_layer_l2 =
        crate::l2::execution_layer::L2ExecutionLayer::new(taiko_config.clone())
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create L2ExecutionLayer: {}", e))?;

    let _node = Node::new(cancel_token).await?;

    let signer = common::signer::create_signer(
        config.web3signer_l2_url.clone(),
        config.catalyst_node_ecdsa_private_key.clone(),
        config.preconfer_address.clone(),
    )
    .await?;

    use common::l2::engine::L2Engine;
    use common::l2::engine::L2EngineConfig;
    use common::l2::taiko_driver::OperationType;
    use common::l2::taiko_driver::TaikoDriver;
    use common::l2::taiko_driver::TaikoDriverConfig;
    use common::shared::{l2_block::L2Block, l2_slot_info::L2SlotInfo};
    use std::time::Duration;

    let l2_engine = L2Engine::new(L2EngineConfig::new(&config, signer.get_address())?)
        .map_err(|e| anyhow::anyhow!("Failed to create L2Engine: {}", e))?;

    let driver_config = TaikoDriverConfig {
        driver_url: config.taiko_driver_url.clone(),
        rpc_driver_preconf_timeout: config.rpc_driver_preconf_timeout,
        rpc_driver_status_timeout: config.rpc_driver_status_timeout,
        jwt_secret_bytes: common::utils::file_operations::read_jwt_secret(
            &config.jwt_secret_file_path,
        )?,
        call_timeout: Duration::from_secs(config.preconf_heartbeat_ms / 2),
    };
    let driver = TaikoDriver::new(&driver_config, metrics).await?;

    loop {
        const BASE_FEE: u64 = 25000000;
        let pending_tx_list = l2_engine
            .get_pending_l2_tx_list(BASE_FEE, 0, 15000000)
            .await?;

        let l2_height = execution_layer_l2.common().get_latest_block_id().await?;

        info!(
            "Pending L2 tx list len: {:?}, L2 height: {l2_height}",
            if let Some(pending_tx_list) = &pending_tx_list {
                pending_tx_list.tx_list.len()
            } else {
                0
            }
        );

        if let Some(pending_tx_list) = pending_tx_list {
            let l2_block = L2Block::new_from(
                pending_tx_list,
                std::time::SystemTime::now() // temp solution
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_secs(),
            );

            const L1_HEIGHT_LAG: u64 = 4;
            let anchor_height = ethereum_l1
                .execution_layer
                .common()
                .get_latest_block_id()
                .await?
                - L1_HEIGHT_LAG;

            let anchor_hash = ethereum_l1
                .execution_layer
                .common()
                .get_block_hash(anchor_height)
                .await?;

            let anchor_block_state_root = ethereum_l1
                .execution_layer
                .common()
                .get_block_state_root_by_number(anchor_height)
                .await?;

            let l2_height = execution_layer_l2.common().get_latest_block_id().await?;
            let l2_hash = execution_layer_l2
                .common()
                .get_block_hash(l2_height)
                .await?;

            l2::advance_head_to_new_l2_block(
                l2_block,
                anchor_height,
                anchor_block_state_root,
                anchor_hash,
                &L2SlotInfo::new(BASE_FEE, 0, l2_height, l2_hash, 0),
                false,
                false,
                OperationType::Preconfirm,
                &driver,
                &execution_layer_l2,
                &signer.get_address(),
                &event_indexer,
            )
            .await?;
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}
