use common::chain_monitor::ChainMonitor;
use taiko_bindings::inbox::Inbox;
//use taiko_bindings::codec_optimized;
use tracing::{info};

pub type ShastaChainMonitor = ChainMonitor<Inbox::Proposed>;

pub fn print_proposed_info(_event: &Inbox::Proposed) {
    // TODO: fix the decoding
    info!("Proposed event → id = ?");
    /*
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
    */
}
