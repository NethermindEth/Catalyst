use crate::l1::execution_layer::ExecutionLayer;
use common::metrics::Metrics;
use common::utils::cancellation_token::CancellationToken;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

struct WhitelistMonitor {
    execution_layer: ExecutionLayer,
    cancel_token: CancellationToken,
    metrics: Arc<Metrics>,
    monitor_interval: Duration,
}

impl WhitelistMonitor {
    pub fn new(
        execution_layer: ExecutionLayer,
        cancel_token: CancellationToken,
        metrics: Arc<Metrics>,
        monitor_interval: Duration,
    ) -> Self {
        Self {
            execution_layer,
            cancel_token,
            metrics,
            monitor_interval,
        }
    }

    pub fn run(self) {
        tokio::spawn(async move {
            self.monitor_whitelist().await;
        });
    }

    async fn monitor_whitelist(self) {
        loop {
            self.execution_layer
                .check_if_operator_is_in_whitelist()
                .await;
            tokio::select! {
                _ = sleep(self.monitor_interval) => {},
                _ = self.cancel_token.cancelled() => {
                    info!("Shutdown signal received, exiting metrics loop...");
                    return;
                }
            }
        }
    }
}
