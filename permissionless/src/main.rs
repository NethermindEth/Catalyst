use anyhow::Error;
use common::utils as common_utils;
use tracing::info;

mod l1;
mod registration;
mod utils;

#[tokio::main]
async fn main() -> Result<(), Error> {
    common_utils::logging::init_logging();

    info!("🚀 Starting URC Node v{}", env!("CARGO_PKG_VERSION"));

    let _config = common_utils::config::Config::<utils::config::Config>::read_env_variables();

    Ok(())
}
