use crate::l1::traits::WhitelistProvider;
use common::metrics::Metrics;
use common::utils::cancellation_token::CancellationToken;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{error, info, warn};

pub struct WhitelistMonitor<T: WhitelistProvider + 'static> {
    execution_layer: Arc<T>,
    cancel_token: CancellationToken,
    metrics: Arc<Metrics>,
    monitor_interval: Duration,
}

impl<T: WhitelistProvider + 'static> WhitelistMonitor<T> {
    pub fn new(
        execution_layer: Arc<T>,
        cancel_token: CancellationToken,
        metrics: Arc<Metrics>,
        monitor_interval_sec: u64,
    ) -> Self {
        Self {
            execution_layer,
            cancel_token,
            metrics,
            monitor_interval: Duration::from_secs(monitor_interval_sec),
        }
    }

    pub fn run(self) {
        tokio::spawn(async move {
            self.monitor_whitelist().await;
        });
    }

    async fn monitor_whitelist(self) {
        loop {
            match self.execution_layer.is_operator_whitelisted().await {
                Ok(is_whitelisted) => {
                    self.metrics.set_operator_whitelisted(is_whitelisted);
                    if !is_whitelisted {
                        warn!("Operator ejected from the whitelist");
                    }
                }
                Err(e) => {
                    error!("Failed to check if operator is whitelisted: {}", e);
                }
            }
            tokio::select! {
                _ = sleep(self.monitor_interval) => {},
                _ = self.cancel_token.cancelled() => {
                    info!("Shutdown signal received, exiting whitelist monitor loop...");
                    return;
                }
            }
        }
    }
}
