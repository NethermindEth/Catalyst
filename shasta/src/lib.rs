mod event_indexer;
mod utils;

use crate::{event_indexer::EventIndexer, utils::config::ShastaConfig};
use anyhow::Error;
use common::{config::Config, config::ConfigTrait, metrics::Metrics};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::info;

pub async fn create_shasta_node(
    config: Config,
    _metrics: Arc<Metrics>,
    _cancel_token: CancellationToken,
) -> Result<(), Error> {
    info!("Creating Shasta node");

    let shasta_config = ShastaConfig::read_env_variables();
    let _event_indexer = EventIndexer::new(
        config
            .l1_rpc_urls
            .first()
            .expect("L1 RPC URL is required")
            .clone(),
        shasta_config.contract_addresses.shasta_inbox.clone(),
    )
    .await?;

    Ok(())
}
