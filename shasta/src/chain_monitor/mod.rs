use alloy::dyn_abi::SolType;
use common::chain_monitor::ChainMonitor;
use taiko_bindings::codec_optimized::IInbox::ProposedEventPayload;
use taiko_bindings::i_inbox::IInbox;
use tracing::info;

pub type ShastaChainMonitor = ChainMonitor<IInbox::Proposed>;

pub fn print_proposed_info(event: &IInbox::Proposed) {
    match <ProposedEventPayload as SolType>::abi_decode(&event.data) {
        Ok(payload) => {
            info!(
                "Proposed event → id = {}, proposer = {}",
                payload.proposal.id, payload.proposal.proposer,
            );
        }
        Err(e) => {
            info!("Failed to decode Proposed event data: {:?}", e);
        }
    }
}
