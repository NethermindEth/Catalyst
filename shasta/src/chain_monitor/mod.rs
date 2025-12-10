use alloy::{primitives::Address, providers::DynProvider};
use common::chain_monitor::{ChainMonitor, ChainMonitorEventHandler};
use taiko_bindings::{codec_optimized::CodecOptimized::CodecOptimizedInstance, inbox::Inbox};
use tracing::{info, warn};

pub type ShastaChainMonitor = ChainMonitor<Inbox::Proposed>;

#[derive(Clone)]
pub struct ProposedHandler {
    codec: CodecOptimizedInstance<DynProvider>,
}

impl ProposedHandler {
    pub fn new(codec_address: Address, provider: DynProvider) -> Self {
        let codec = CodecOptimizedInstance::new(codec_address, provider);
        Self { codec }
    }
}

impl ChainMonitorEventHandler<Inbox::Proposed> for ProposedHandler {
    fn handle_event(&self, event: &Inbox::Proposed) {
        let cloned = self.clone();
        let event_data = event.data.clone();

        // Spawn a blocking task to run the async code
        tokio::task::spawn(async move {
            match cloned.codec.decodeProposedEvent(event_data).call().await {
                Ok(payload) => {
                    info!(
                        "Proposed event → id = {}, proposer = {}, timestamp = {}",
                        payload.proposal.id, payload.proposal.proposer, payload.proposal.timestamp
                    );
                }
                Err(e) => {
                    warn!("Failed to decode Proposed event data: {:?}", e);
                }
            }
        });
    }
}
