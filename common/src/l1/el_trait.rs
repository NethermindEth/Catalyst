use crate::l1::transaction_error::TransactionError;

use crate::execution_layer::ExecutionLayer;
use crate::metrics::Metrics;
use alloy::primitives::U256;
use anyhow::Error;
use std::future::Future;
use std::marker::Send;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;

/// Execution layer trait.
/// Enables additional features to the execution layer, specific for permissionless or whitelist implementation.
pub trait ELTrait: Send + Sync + Sized {
    type Config;
    fn new(
        common_config: super::config::EthereumL1Config,
        specific_config: Self::Config,
        transaction_error_channel: Sender<TransactionError>,
        metrics: Arc<Metrics>,
    ) -> impl std::future::Future<Output = Result<Self, Error>> + Send;

    fn common(&self) -> &ExecutionLayer;
    fn get_preconfer_total_bonds(&self) -> impl Future<Output = Result<U256, Error>> + Send;
}
