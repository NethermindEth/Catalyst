use super::execution_layer_inner::ExecutionLayerInner;
use alloy::providers::DynProvider;
use std::sync::Arc;

/// Execution layer extension trait.
/// Enables additional features to the execution layer, specific for URC or whitelist implementation.
pub trait ELExtension: Send + Sync {
    type Config;
    fn new(inner: Arc<ExecutionLayerInner>, provider: DynProvider, config: Self::Config) -> Self;
}
