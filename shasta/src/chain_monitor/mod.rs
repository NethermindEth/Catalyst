use common::chain_monitor::ChainMonitor;
use taiko_bindings::i_inbox::IInbox;
use taiko_protocol::shasta::codec_optimized;
use tracing::{info, warn};

pub type ShastaChainMonitor = ChainMonitor<IInbox::Proposed>;

pub fn print_proposed_info(event: &IInbox::Proposed) {
    match codec_optimized::decode_proposed_event(&event.data) {
        Ok(payload) => {
            info!(
                "Proposed event → id = {}, proposer = {}",
                payload.proposal.id, payload.proposal.proposer,
            );
        }
        Err(e) => {
            warn!("Failed to decode Proposed event data: {:?}", e);
        }
    }
}
