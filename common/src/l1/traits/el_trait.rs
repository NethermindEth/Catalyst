use crate::l1::transaction_error::TransactionError;

use crate::metrics::Metrics;
use crate::shared::execution_layer::ExecutionLayer;
use anyhow::Error;
use std::marker::Send;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;

/// Execution layer trait.
/// Enables additional features to the execution layer, specific for permissionless or whitelist implementation.
pub trait ELTrait: Send + Sync + Sized {
    type Config;
    fn new(
        common_config: crate::l1::config::EthereumL1Config,
        specific_config: Self::Config,
        transaction_error_channel: Sender<TransactionError>,
        metrics: Arc<Metrics>,
    ) -> impl std::future::Future<Output = Result<Self, Error>> + Send;

    fn common(&self) -> &ExecutionLayer;
}
