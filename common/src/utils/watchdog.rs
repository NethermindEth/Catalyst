use crate::utils::cancellation_token::CancellationToken;
use tracing::error;

pub struct Watchdog {
    counter: u64,
    max_counter: u64,
    cancel_token: CancellationToken,
}

impl Watchdog {
    pub fn new(cancel_token: CancellationToken, max_counter: u64) -> Self {
        Self {
            counter: 0,
            max_counter,
            cancel_token,
        }
    }

    pub fn reset(&mut self) {
        self.counter = 0;
    }

    pub fn increment(&mut self) {
        self.counter += 1;
        if self.counter > self.max_counter {
            error!(
                "Watchdog triggered after {} heartbeats, shutting down...",
                self.counter
            );
            self.cancel_token.cancel_on_critical_error();
        }
    }
}
