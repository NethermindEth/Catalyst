use crate::l1::bindings::taiko_inbox::ITaikoInbox;
use common::chain_monitor::{ChainMonitor, ChainMonitorEventHandler};
use tracing::info;

mod whitelist_monitor;
pub use whitelist_monitor::WhitelistMonitor;

pub type PacayaChainMonitor = ChainMonitor<ITaikoInbox::BatchProposed>;

#[derive(Clone)]
pub struct BatchProposedHandler;

impl ChainMonitorEventHandler<ITaikoInbox::BatchProposed> for BatchProposedHandler {
    fn handle_event(&self, event: &ITaikoInbox::BatchProposed) {
        info!(
            "BatchProposed event → lastBlockId = {}, coinbase = {}",
            event.info.lastBlockId, event.info.coinbase,
        );
    }
}
