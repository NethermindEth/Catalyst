use common::chain_monitor::{ChainMonitor, ChainMonitorEventHandler};
use taiko_bindings::inbox::Inbox;
use tracing::info;

pub type ShastaChainMonitor = ChainMonitor<Inbox::Proposed>;

#[derive(Clone)]
pub struct ProposedHandler {}

impl ProposedHandler {
    pub fn new() -> Self {
        Self {}
    }
}

impl ChainMonitorEventHandler<Inbox::Proposed> for ProposedHandler {
    fn handle_event(&self, event: &Inbox::Proposed) {
        info!(
            "Proposed event → id = {}, proposer = {}, end of submission window timestamp = {}",
            event.id, event.proposer, event.endOfSubmissionWindowTimestamp
        );
    }
}
