use super::proposal_manager::ProposalManager;
use crate::{
    l1::execution_layer::ExecutionLayer,
    l2::taiko::Taiko,
    metrics::Metrics,
    node::{LastSafeL2BlockFinder, proposal_manager::proposal::Proposals},
};
use alloy::primitives::B256;
use anyhow::Error;
use common::{
    l1::ethereum_l1::EthereumL1, utils::cancellation_token::CancellationToken, utils::types::*,
};
use std::{cmp::Ordering, sync::Arc};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

pub enum VerificationResult {
    SuccessNoProposals,
    SuccessWithProposals(Proposals),
    ReanchorNeeded(u64, String),
    SlotNotValid,
    VerificationInProgress,
}

#[derive(Clone)]
struct PreconfirmationRootBlock {
    number: u64,
    hash: B256,
}

pub struct Verifier {
    verification_slot: Slot,
    verifier_thread: Option<VerifierThread>,
    verifier_thread_handle: Option<JoinHandle<Result<Proposals, Error>>>,
    last_safe_l2_block_finder: Arc<LastSafeL2BlockFinder>,
}

struct VerifierThread {
    taiko: Arc<Taiko>,
    preconfirmation_root: PreconfirmationRootBlock,
    proposal_manager: ProposalManager,
    cancel_token: CancellationToken,
}

impl Verifier {
    pub async fn new_with_taiko_height(
        taiko_geth_height: u64,
        taiko: Arc<Taiko>,
        proposal_manager: ProposalManager,
        verification_slot: Slot,
        cancel_token: CancellationToken,
        last_safe_l2_block_finder: Arc<LastSafeL2BlockFinder>,
    ) -> Result<Self, Error> {
        let hash = taiko.get_l2_block_hash(taiko_geth_height).await?;
        debug!(
            "Verifier created with taiko_geth_height: {}, hash: {}, verification_slot: {}",
            taiko_geth_height, hash, verification_slot
        );
        let preconfirmation_root = PreconfirmationRootBlock {
            number: taiko_geth_height,
            hash,
        };
        Ok(Self {
            verifier_thread: Some(VerifierThread {
                taiko,
                preconfirmation_root: preconfirmation_root.clone(),
                proposal_manager,
                cancel_token,
            }),
            verification_slot,
            verifier_thread_handle: None,
            last_safe_l2_block_finder,
        })
    }

    pub fn is_slot_valid(&self, current_slot: Slot) -> bool {
        current_slot >= self.verification_slot
    }

    pub fn get_verification_slot(&self) -> Slot {
        self.verification_slot
    }

    async fn start_verification_thread(&mut self, taiko_inbox_height: u64, metrics: Arc<Metrics>) {
        if let Some(mut verifier_thread) = self.verifier_thread.take() {
            self.verifier_thread_handle = Some(tokio::spawn(async move {
                info!("🔍 Started block verification thread");

                verifier_thread
                    .verify_submitted_blocks(taiko_inbox_height, metrics)
                    .await
            }));
        } else {
            warn!("Verifier thread already started");
        }
    }

    /// Returns true if the operation succeeds
    pub async fn verify(
        &mut self,
        ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
        metrics: Arc<Metrics>,
    ) -> Result<VerificationResult, Error> {
        if let Some(handle) = self.verifier_thread_handle.as_mut() {
            if handle.is_finished() {
                debug!("Verifier thread handle has finished");
                let result = handle.await?;
                match result {
                    Ok(proposals) => {
                        debug!("Proposals to send from verifier: {}", proposals.len());
                        if proposals.is_empty() {
                            return Ok(VerificationResult::SuccessNoProposals);
                        }
                        Ok(VerificationResult::SuccessWithProposals(proposals))
                    }
                    Err(err) => {
                        let taiko_inbox_height = self.last_safe_l2_block_finder.get().await?;
                        Ok(VerificationResult::ReanchorNeeded(
                            taiko_inbox_height,
                            format!("Verifier return an error: {err}"),
                        ))
                    }
                }
            } else {
                Ok(VerificationResult::VerificationInProgress)
            }
        } else {
            let head_slot = ethereum_l1.consensus_layer.get_head_slot_number().await?;

            if !self.is_slot_valid(head_slot) {
                info!(
                    "Slot {} is not valid for verification, target slot {}, skipping",
                    head_slot,
                    self.get_verification_slot()
                );
                return Ok(VerificationResult::SlotNotValid);
            }

            let taiko_inbox_height = self.last_safe_l2_block_finder.get().await?;
            self.start_verification_thread(taiko_inbox_height, metrics)
                .await;

            Ok(VerificationResult::VerificationInProgress)
        }
    }
}

impl VerifierThread {
    async fn verify_submitted_blocks(
        &mut self,
        taiko_inbox_height: u64,
        metrics: Arc<Metrics>,
    ) -> Result<Proposals, Error> {
        // Compare block hashes to confirm that the block is still the same.
        // If not, return an error that will trigger a reorg.
        let current_hash = self
            .taiko
            .get_l2_block_hash(self.preconfirmation_root.number)
            .await?;
        if self.preconfirmation_root.hash != current_hash {
            return Err(anyhow::anyhow!(
                "❌ Block {} hash mismatch: current: {}, expected: {}",
                self.preconfirmation_root.number,
                current_hash,
                self.preconfirmation_root.hash
            ));
        }

        match self.preconfirmation_root.number.cmp(&taiko_inbox_height) {
            Ordering::Greater => {
                // preconfirmation_root.number > taiko_inbox_height
                // make proposals from blocks unprocessed by previous preconfer
                info!(
                    "Taiko geth has {} blocks more than Taiko Inbox. Preparing proposal for submission.",
                    self.preconfirmation_root.number - taiko_inbox_height
                );

                self.handle_unprocessed_blocks(
                    taiko_inbox_height,
                    self.preconfirmation_root.number,
                )
                .await?;
            }
            Ordering::Less => {
                // preconfirmation_root.number < taiko_inbox_height
                // extra block proposal was made by previous preconfer
                // return an error that will trigger a reorg.
                return Err(anyhow::anyhow!(
                    "❌ Unexpected block proposal was made by previous preconfer: preconfirming on {} but taiko_inbox_height is {}",
                    self.preconfirmation_root.number,
                    taiko_inbox_height
                ));
            }
            Ordering::Equal => {
                // preconfirmation_root.number == taiko_inbox_height
                // all good
            }
        }
        info!(
            "🔍 Verified block successfully: preconfirmation_root {}, hash: {} ",
            self.preconfirmation_root.number, self.preconfirmation_root.hash
        );

        metrics.inc_by_batch_recovered(self.proposal_manager.get_number_of_proposals());

        self.proposal_manager.try_finalize_current_proposal()?;
        Ok(self.proposal_manager.take_proposals_to_send())
    }

    async fn handle_unprocessed_blocks(
        &mut self,
        taiko_inbox_height: u64,
        taiko_geth_height: u64,
    ) -> Result<(), Error> {
        let start = std::time::Instant::now();

        let first_block = taiko_inbox_height + 1;
        let (anchor_offset, timestamp_offset) = self
            .proposal_manager
            .get_l1_anchor_block_and_timestamp_offset_for_l2_block(first_block)
            .await?;

        if !self
            .proposal_manager
            .is_offsets_valid(anchor_offset, timestamp_offset)
        {
            return Err(anyhow::anyhow!(
                "Offset exceeded during recovery at block {}: anchor_offset={}, timestamp_offset={}",
                first_block,
                anchor_offset,
                timestamp_offset,
            ));
        }

        let mut parent_timestamp = self
            .proposal_manager
            .validate_block_timestamp(taiko_inbox_height, 0)
            .await?;

        // Sync FI with L1 chain
        self.proposal_manager.reset_builder().await?;

        for current_height in first_block..=taiko_geth_height {
            if self.cancel_token.is_cancelled() {
                return Err(anyhow::anyhow!("Verification cancelled"));
            }

            parent_timestamp = self
                .proposal_manager
                .validate_block_timestamp(current_height, parent_timestamp)
                .await?;

            self.proposal_manager
                .recover_from_l2_block(current_height)
                .await?;
        }

        let elapsed = start.elapsed().as_millis();
        info!("Recovered in {} milliseconds", elapsed);

        Ok(())
    }
}
