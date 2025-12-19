use common::chain_monitor::ChainMonitor;
use taiko_bindings::inbox::Inbox;
use tracing::info;

pub type ShastaChainMonitor = ChainMonitor<Inbox::Proposed>;

pub fn print_proposed_info(event: &Inbox::Proposed) {
    info!(
        "Proposed event → id = {}, proposer = {}, end of submission window timestamp = {}",
        event.id, event.proposer, event.endOfSubmissionWindowTimestamp
    );
}
