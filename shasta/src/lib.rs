#[allow(dead_code)] // TODO: remove this once we have a used create_shasta_node function
mod utils;

use crate::utils::config::ShastaConfig;
use anyhow::Error;
use common::{config::Config, config::ConfigTrait, metrics::Metrics};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::info;

pub async fn create_shasta_node(
    _config: Config,
    _metrics: Arc<Metrics>,
    _cancel_token: CancellationToken,
) -> Result<(), Error> {
    info!("Creating Shasta node");

    let _shasta_config = ShastaConfig::read_env_variables();

    Ok(())
}
