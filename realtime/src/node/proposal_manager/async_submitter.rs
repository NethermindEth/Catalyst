use crate::l1::execution_layer::ExecutionLayer;
use crate::node::proposal_manager::bridge_handler::{UserOpStatus, UserOpStatusStore};
use crate::node::proposal_manager::proposal::Proposal;
use crate::raiko::{RaikoCheckpoint, RaikoClient, RaikoProofRequest};
use alloy::primitives::B256;
use anyhow::Error;
use common::l1::ethereum_l1::EthereumL1;
use std::sync::Arc;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::info;

pub struct SubmissionResult {
    pub new_parent_proposal_hash: B256,
}

struct InFlightSubmission {
    result_rx: oneshot::Receiver<Result<SubmissionResult, Error>>,
    handle: JoinHandle<()>,
}

pub struct AsyncSubmitter {
    in_flight: Option<InFlightSubmission>,
    raiko_client: RaikoClient,
    basefee_sharing_pctg: u8,
    ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
}

impl AsyncSubmitter {
    pub fn new(
        raiko_client: RaikoClient,
        basefee_sharing_pctg: u8,
        ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
    ) -> Self {
        Self {
            in_flight: None,
            raiko_client,
            basefee_sharing_pctg,
            ethereum_l1,
        }
    }

    pub fn is_busy(&self) -> bool {
        self.in_flight.is_some()
    }

    /// Non-blocking check for completed submission. Returns None if idle or still in progress.
    pub fn try_recv_result(&mut self) -> Option<Result<SubmissionResult, Error>> {
        let in_flight = self.in_flight.as_mut()?;
        match in_flight.result_rx.try_recv() {
            Ok(result) => {
                self.in_flight = None;
                Some(result)
            }
            Err(oneshot::error::TryRecvError::Empty) => None,
            Err(oneshot::error::TryRecvError::Closed) => {
                self.in_flight = None;
                Some(Err(anyhow::anyhow!(
                    "Submission task panicked or was dropped"
                )))
            }
        }
    }

    /// Submit a proposal asynchronously. Spawns a background task that fetches the ZK proof
    /// from Raiko and then sends the L1 transaction. Results are retrieved via `try_recv_result`.
    pub fn submit(&mut self, proposal: Proposal, status_store: Option<UserOpStatusStore>) {
        assert!(
            !self.is_busy(),
            "Cannot submit while another submission is in flight"
        );

        let (result_tx, result_rx) = oneshot::channel();
        let raiko_client = self.raiko_client.clone();
        let basefee_sharing_pctg = self.basefee_sharing_pctg;
        let ethereum_l1 = self.ethereum_l1.clone();

        let handle = tokio::spawn(async move {
            let result = submission_task(
                proposal,
                &raiko_client,
                basefee_sharing_pctg,
                ethereum_l1,
                status_store,
            )
            .await;
            let _ = result_tx.send(result);
        });

        self.in_flight = Some(InFlightSubmission { result_rx, handle });
    }

    pub fn abort(&mut self) {
        if let Some(in_flight) = self.in_flight.take() {
            in_flight.handle.abort();
        }
    }
}

async fn submission_task(
    mut proposal: Proposal,
    raiko_client: &RaikoClient,
    basefee_sharing_pctg: u8,
    ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
    status_store: Option<UserOpStatusStore>,
) -> Result<SubmissionResult, Error> {
    // Step 1: Fetch ZK proof from Raiko
    if proposal.zk_proof.is_none() {
        info!(
            "Fetching ZK proof from Raiko for batch with {} blocks",
            proposal.l2_blocks.len()
        );

        let l2_block_numbers: Vec<u64> = (proposal.checkpoint.blockNumber.to::<u64>()
            - u64::try_from(proposal.l2_blocks.len())? + 1
            ..=proposal.checkpoint.blockNumber.to::<u64>())
            .collect();

        let request = RaikoProofRequest {
            l2_block_numbers,
            proof_type: raiko_client.proof_type.clone(),
            max_anchor_block_number: proposal.max_anchor_block_number,
            parent_proposal_hash: format!("0x{}", hex::encode(proposal.parent_proposal_hash)),
            basefee_sharing_pctg,
            network: None,
            l1_network: None,
            prover: None,
            signal_slots: proposal
                .signal_slots
                .iter()
                .map(|s| format!("0x{}", hex::encode(s)))
                .collect(),
            sources: vec![],
            checkpoint: Some(RaikoCheckpoint {
                block_number: proposal.checkpoint.blockNumber.to::<u64>(),
                block_hash: format!("0x{}", hex::encode(proposal.checkpoint.blockHash)),
                state_root: format!("0x{}", hex::encode(proposal.checkpoint.stateRoot)),
            }),
            blob_proof_type: "ProofOfEquivalence".to_string(),
        };

        let proof = raiko_client.get_proof(&request).await?;
        proposal.zk_proof = Some(proof);
    }

    // Step 2: Send L1 transaction
    let user_op_ids: Vec<u64> = proposal.user_ops.iter().map(|op| op.id).collect();
    let has_user_ops = !user_op_ids.is_empty() && status_store.is_some();

    let (tx_hash_sender, tx_hash_receiver) = if has_user_ops {
        let (s, r) = tokio::sync::oneshot::channel();
        (Some(s), Some(r))
    } else {
        (None, None)
    };
    let (tx_result_sender, tx_result_receiver) = if has_user_ops {
        let (s, r) = tokio::sync::oneshot::channel();
        (Some(s), Some(r))
    } else {
        (None, None)
    };

    if let Err(err) = ethereum_l1
        .execution_layer
        .send_batch_to_l1(proposal.clone(), tx_hash_sender, tx_result_sender)
        .await
    {
        // Mark user ops as rejected on failure
        if let Some(ref store) = status_store {
            let reason = format!("L1 multicall failed: {}", err);
            for op in &proposal.user_ops {
                store.set(
                    op.id,
                    &UserOpStatus::Rejected {
                        reason: reason.clone(),
                    },
                );
            }
        }
        return Err(err);
    }

    // Step 3: Compute new parent proposal hash
    let new_parent_proposal_hash = alloy::primitives::keccak256(
        &alloy::sol_types::SolValue::abi_encode(&(
            proposal.parent_proposal_hash,
            proposal.max_anchor_block_number,
            proposal.max_anchor_block_hash,
        )),
    );

    // Step 4: Spawn user-op status tracker
    if let (Some(hash_rx), Some(result_rx), Some(store)) =
        (tx_hash_receiver, tx_result_receiver, status_store)
    {
        tokio::spawn(async move {
            let tx_hash = match hash_rx.await {
                Ok(tx_hash) => {
                    for id in &user_op_ids {
                        store.set(*id, &UserOpStatus::Processing { tx_hash });
                    }
                    Some(tx_hash)
                }
                Err(_) => {
                    for id in &user_op_ids {
                        store.set(
                            *id,
                            &UserOpStatus::Rejected {
                                reason: "Transaction failed to send".to_string(),
                            },
                        );
                    }
                    None
                }
            };

            if tx_hash.is_some() {
                match result_rx.await {
                    Ok(true) => {
                        for id in &user_op_ids {
                            store.set(*id, &UserOpStatus::Executed);
                        }
                    }
                    Ok(false) => {
                        for id in &user_op_ids {
                            store.set(
                                *id,
                                &UserOpStatus::Rejected {
                                    reason: "L1 multicall reverted".to_string(),
                                },
                            );
                        }
                    }
                    Err(_) => {
                        for id in &user_op_ids {
                            store.set(
                                *id,
                                &UserOpStatus::Rejected {
                                    reason: "Transaction monitor dropped".to_string(),
                                },
                            );
                        }
                    }
                }
            }
        });
    }

    Ok(SubmissionResult {
        new_parent_proposal_hash,
    })
}
