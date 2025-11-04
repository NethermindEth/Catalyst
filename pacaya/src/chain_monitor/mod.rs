use crate::l1::bindings::taiko_inbox::ITaikoInbox;
use common::chain_monitor::ChainMonitor;
use tracing::info;

// Type alias for BatchProposed events for backward compatibility
pub type PacayaChainMonitor = ChainMonitor<ITaikoInbox::BatchProposed>;

// Example event handler function
pub fn print_batch_proposed_info(event: &ITaikoInbox::BatchProposed) {
    info!(
        "BatchProposed event → lastBlockId = {}, coinbase = {}",
        event.info.lastBlockId,
        event.info.coinbase,
    );
}
