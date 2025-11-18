use crate::metrics::Metrics;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::error;

pub struct Watchdog {
    counter: u64,
    max_counter: u64,
    cancel_token: CancellationToken,
    metrics: Arc<Metrics>,
}

impl Watchdog {
    pub fn new(cancel_token: CancellationToken, max_counter: u64, metrics: Arc<Metrics>) -> Self {
        Self {
            counter: 0,
            max_counter,
            cancel_token,
            metrics,
        }
    }

    pub fn reset(&mut self) {
        self.counter = 0;
    }

    pub fn increment(&mut self) {
        self.counter += 1;
        if self.counter > self.max_counter {
            self.metrics.inc_critical_errors();
            error!(
                "Watchdog triggered after {} heartbeats, shutting down...",
                self.counter
            );
            self.cancel_token.cancel();
        }
    }
}
