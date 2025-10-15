use alloy::{primitives::Address, transports::http::reqwest::Url};
use anyhow::Error;
use std::{str::FromStr, sync::Arc};
use taiko_event_indexer::{
    indexer::{ShastaEventIndexer, ShastaEventIndexerConfig},
    interface::ShastaProposeInputReader,
};
use taiko_rpc::SubscriptionSource;
use tracing::debug;

#[allow(dead_code)] // TODO: remove this once we have a used event_indexer field
pub struct EventIndexer {
    indexer: Arc<ShastaEventIndexer>,
}

impl EventIndexer {
    pub async fn new(l1_ws_rpc_url: String, inbox_contract_address: String) -> Result<Self, Error> {
        let config = ShastaEventIndexerConfig {
            l1_subscription_source: SubscriptionSource::Ws(Url::from_str(l1_ws_rpc_url.as_str())?),
            inbox_address: Address::from_str(&inbox_contract_address)?,
        };

        let indexer = ShastaEventIndexer::new(config).await?;
        indexer.clone().spawn();
        debug!("Spawned Shasta event indexer");
        indexer.wait_historical_indexing_finished().await;

        let propose_input = indexer.read_shasta_propose_input();
        if let Some(propose_input) = propose_input {
            debug!(
                "core_state.nextProposalId: {:?}",
                propose_input.core_state.nextProposalId
            );
            debug!(
                "core_state.lastProposalBlockId: {:?}",
                propose_input.core_state.lastProposalBlockId
            );
            debug!(
                "core_state.lastFinalizedProposalId: {:?}",
                propose_input.core_state.lastFinalizedProposalId
            );
            debug!(
                "core_state.lastCheckpointTimestamp: {:?}",
                propose_input.core_state.lastCheckpointTimestamp
            );
            debug!(
                "core_state.lastFinalizedTransitionHash: {:?}",
                propose_input.core_state.lastFinalizedTransitionHash
            );
            debug!(
                "core_state.bondInstructionsHash: {:?}",
                propose_input.core_state.bondInstructionsHash
            );

            for (idx, proposal) in propose_input.proposals.iter().enumerate() {
                debug!("proposal[{}].id: {:?}", idx, proposal.id);
                debug!("proposal[{}].timestamp: {:?}", idx, proposal.timestamp);
                debug!(
                    "proposal[{}].endOfSubmissionWindowTimestamp: {:?}",
                    idx, proposal.endOfSubmissionWindowTimestamp
                );
                debug!("proposal[{}].proposer: {:?}", idx, proposal.proposer);
                debug!(
                    "proposal[{}].coreStateHash: {:?}",
                    idx, proposal.coreStateHash
                );
                debug!(
                    "proposal[{}].derivationHash: {:?}",
                    idx, proposal.derivationHash
                );
            }

            for (idx, tr) in propose_input.transition_records.iter().enumerate() {
                debug!("transition_record[{}].span: {:?}", idx, tr.span);
                debug!(
                    "transition_record[{}].bondInstructions: {:?}",
                    idx, tr.bondInstructions
                );
                debug!(
                    "transition_record[{}].transitionHash: {:?}",
                    idx, tr.transitionHash
                );
                debug!(
                    "transition_record[{}].checkpointHash: {:?}",
                    idx, tr.checkpointHash
                );
            }

            debug!("checkpoint: {:?}", propose_input.checkpoint);
        }

        Ok(Self { indexer })
    }
}
