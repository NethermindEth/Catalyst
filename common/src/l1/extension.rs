use super::{config::ProtocolConfig, execution_layer_inner::ExecutionLayerInner};
use alloy::{primitives::U256, providers::DynProvider};
use anyhow::Error;
use std::future::Future;
use std::marker::Send;
use std::sync::Arc;

/// Execution layer extension trait.
/// Enables additional features to the execution layer, specific for URC or whitelist implementation.
pub trait ELExtension: Send + Sync {
    type Config;
    fn new(
        inner: Arc<ExecutionLayerInner>,
        provider: DynProvider,
        config: Self::Config,
    ) -> impl std::future::Future<Output = Self> + Send;

    fn get_preconfer_total_bonds(&self) -> impl Future<Output = Result<U256, Error>> + Send;
    fn fetch_protocol_config(&self) -> impl Future<Output = Result<ProtocolConfig, Error>> + Send;
}
