use crate::l1::bindings::taiko_inbox::ITaikoInbox;
use common::chain_monitor::ChainMonitor;
use tracing::info;

mod whitelist_monitor;
pub use whitelist_monitor::WhitelistMonitor;

pub type PacayaChainMonitor = ChainMonitor<ITaikoInbox::BatchProposed>;

pub fn print_batch_proposed_info(event: &ITaikoInbox::BatchProposed) {
    info!(
        "BatchProposed event → lastBlockId = {}, coinbase = {}",
        event.info.lastBlockId, event.info.coinbase,
    );
}
