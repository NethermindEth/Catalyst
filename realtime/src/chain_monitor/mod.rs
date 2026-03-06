use crate::l1::bindings::RealTimeInbox;
use common::chain_monitor::ChainMonitor;
use tracing::info;

pub type RealtimeChainMonitor = ChainMonitor<RealTimeInbox::ProposedAndProved>;

pub fn print_proposed_and_proved_info(event: &RealTimeInbox::ProposedAndProved) {
    info!(
        "ProposedAndProved event → proposalHash = {}, parentProposalHash = {}, maxAnchorBlockNumber = {}",
        event.proposalHash, event.parentProposalHash, event.maxAnchorBlockNumber
    );
}
