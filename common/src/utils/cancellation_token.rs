use crate::metrics::Metrics;
use std::sync::Arc;
use tracing::error;

#[derive(Clone)]
pub struct CancellationToken {
    cancel_token: tokio_util::sync::CancellationToken,
    metrics: Arc<Metrics>,
}

impl CancellationToken {
    pub fn new(metrics: Arc<Metrics>) -> Self {
        Self {
            cancel_token: tokio_util::sync::CancellationToken::new(),
            metrics,
        }
    }
}

impl CancellationToken {
    pub fn cancel(&self) {
        self.cancel_token.cancel();
    }

    pub fn cancel_on_critical_error(&self) {
        error!("Critical error occurred, cancelling token");
        self.metrics.inc_critical_errors();
        self.cancel_token.cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel_token.is_cancelled()
    }

    pub fn cancelled(&self) -> tokio_util::sync::WaitForCancellationFuture<'_> {
        self.cancel_token.cancelled()
    }
}
