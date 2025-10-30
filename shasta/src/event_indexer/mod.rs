use alloy::{primitives::Address, transports::http::reqwest::Url};
use anyhow::Error;
use std::{str::FromStr, sync::Arc};
use taiko_event_indexer::{
    indexer::{ShastaEventIndexer, ShastaEventIndexerConfig},
    interface::{ShastaProposeInput, ShastaProposeInputReader},
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

        Ok(Self { indexer })
    }

    pub fn get_propose_input(&self) -> Option<ShastaProposeInput> {
        let propose_input = self.indexer.read_shasta_propose_input();
        self.debug_propose_input(&propose_input);
        propose_input
    }

    fn debug_propose_input(&self, propose_input: &Option<ShastaProposeInput>) {
        if let Some(propose_input) = propose_input {
            let mut msg = String::new();
            use std::fmt::Write;

            writeln!(
                &mut msg,
                "core_state.nextProposalId: {:?}",
                propose_input.core_state.nextProposalId
            )
            .ok();
            writeln!(
                &mut msg,
                "core_state.lastProposalBlockId: {:?}",
                propose_input.core_state.lastProposalBlockId
            )
            .ok();
            writeln!(
                &mut msg,
                "core_state.lastFinalizedProposalId: {:?}",
                propose_input.core_state.lastFinalizedProposalId
            )
            .ok();
            writeln!(
                &mut msg,
                "core_state.lastCheckpointTimestamp: {:?}",
                propose_input.core_state.lastCheckpointTimestamp
            )
            .ok();
            writeln!(
                &mut msg,
                "core_state.lastFinalizedTransitionHash: {:?}",
                propose_input.core_state.lastFinalizedTransitionHash
            )
            .ok();
            writeln!(
                &mut msg,
                "core_state.bondInstructionsHash: {:?}",
                propose_input.core_state.bondInstructionsHash
            )
            .ok();

            for (idx, proposal) in propose_input.proposals.iter().enumerate() {
                writeln!(&mut msg, "proposal[{}].id: {:?}", idx, proposal.id).ok();
                writeln!(
                    &mut msg,
                    "proposal[{}].timestamp: {:?}",
                    idx, proposal.timestamp
                )
                .ok();
                writeln!(
                    &mut msg,
                    "proposal[{}].endOfSubmissionWindowTimestamp: {:?}",
                    idx, proposal.endOfSubmissionWindowTimestamp
                )
                .ok();
                writeln!(
                    &mut msg,
                    "proposal[{}].proposer: {:?}",
                    idx, proposal.proposer
                )
                .ok();
                writeln!(
                    &mut msg,
                    "proposal[{}].coreStateHash: {:?}",
                    idx, proposal.coreStateHash
                )
                .ok();
                writeln!(
                    &mut msg,
                    "proposal[{}].derivationHash: {:?}",
                    idx, proposal.derivationHash
                )
                .ok();
            }

            for (idx, tr) in propose_input.transition_records.iter().enumerate() {
                writeln!(&mut msg, "transition_record[{}].span: {:?}", idx, tr.span).ok();
                writeln!(
                    &mut msg,
                    "transition_record[{}].bondInstructions: {:?}",
                    idx, tr.bondInstructions
                )
                .ok();
                writeln!(
                    &mut msg,
                    "transition_record[{}].transitionHash: {:?}",
                    idx, tr.transitionHash
                )
                .ok();
                writeln!(
                    &mut msg,
                    "transition_record[{}].checkpointHash: {:?}",
                    idx, tr.checkpointHash
                )
                .ok();
            }

            writeln!(&mut msg, "checkpoint: {:?}", propose_input.checkpoint).ok();

            debug!("{}", msg);
        }
    }
}
