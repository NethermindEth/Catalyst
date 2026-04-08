use crate::l1::execution_layer::ExecutionLayer;
use crate::node::proposal_manager::bridge_handler::{UserOpStatus, UserOpStatusStore};
use crate::node::proposal_manager::proposal::Proposal;
use crate::raiko::{
    RaikoBlobSlice, RaikoCheckpoint, RaikoClient, RaikoDerivationSource, RaikoProofRequest,
};
use alloy::consensus::SidecarBuilder;
use alloy::primitives::B256;
use anyhow::Error;
use common::l1::ethereum_l1::EthereumL1;
use std::sync::Arc;
use taiko_protocol::shasta::BlobCoder;
use taiko_protocol::shasta::manifest::{BlockManifest, DerivationSourceManifest};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::info;

pub struct SubmissionResult {
    pub new_last_finalized_block_hash: B256,
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
    proof_request_bypass: bool,
}

impl AsyncSubmitter {
    pub fn new(
        raiko_client: RaikoClient,
        basefee_sharing_pctg: u8,
        ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
        proof_request_bypass: bool,
    ) -> Self {
        Self {
            in_flight: None,
            raiko_client,
            basefee_sharing_pctg,
            ethereum_l1,
            proof_request_bypass,
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
        let proof_request_bypass = self.proof_request_bypass;

        let handle = tokio::spawn(async move {
            let result = submission_task(
                proposal,
                &raiko_client,
                basefee_sharing_pctg,
                ethereum_l1,
                status_store,
                proof_request_bypass,
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
    proof_request_bypass: bool,
) -> Result<SubmissionResult, Error> {
    // Step 1: Fetch ZK proof from Raiko (or bypass)
    if proposal.zk_proof.is_none() {
        let l2_block_numbers: Vec<u64> =
            (proposal.checkpoint.blockNumber.to::<u64>() - u64::try_from(proposal.l2_blocks.len())?
                + 1..=proposal.checkpoint.blockNumber.to::<u64>())
                .collect();

        // Build the blob sidecar (same as proposal_tx_builder) to get blob hashes and raw data
        let mut block_manifests = Vec::with_capacity(proposal.l2_blocks.len());
        for l2_block in &proposal.l2_blocks {
            block_manifests.push(BlockManifest {
                timestamp: l2_block.timestamp_sec,
                coinbase: l2_block.coinbase,
                anchor_block_number: l2_block.anchor_block_number,
                gas_limit: l2_block.gas_limit_without_anchor,
                transactions: l2_block
                    .prebuilt_tx_list
                    .tx_list
                    .iter()
                    .map(|tx| tx.clone().into())
                    .collect(),
            });
        }
        let manifest = DerivationSourceManifest {
            blocks: block_manifests,
        };
        let manifest_data = manifest.encode_and_compress()?;
        let sidecar_builder: SidecarBuilder<BlobCoder> = SidecarBuilder::from_slice(&manifest_data);
        let sidecar: alloy::eips::eip4844::BlobTransactionSidecar = sidecar_builder.build()?;

        // Extract versioned blob hashes
        let blob_hashes: Vec<String> = sidecar
            .versioned_hashes()
            .map(|h| format!("0x{}", hex::encode(h)))
            .collect();

        // Extract raw blob data (each blob is 131072 bytes, hex-encoded with 0x prefix)
        let blobs: Vec<String> = sidecar
            .blobs
            .iter()
            .map(|blob| format!("0x{}", hex::encode::<&[u8]>(blob.as_ref())))
            .collect();

        // Build sources array with a single DerivationSource entry
        let sources = vec![RaikoDerivationSource {
            is_forced_inclusion: false,
            blob_slice: RaikoBlobSlice {
                blob_hashes,
                offset: 0,
                timestamp: 0,
            },
        }];

        let l2_block_hashes: Vec<String> = proposal
            .l2_block_hashes
            .iter()
            .map(|h| format!("0x{}", hex::encode(h)))
            .collect();

        let request = RaikoProofRequest {
            l2_block_numbers,
            l2_block_hashes: Some(l2_block_hashes),
            proof_type: raiko_client.proof_type.raiko_proof_type().to_string(),
            max_anchor_block_number: proposal.max_anchor_block_number,
            last_finalized_block_hash: format!(
                "0x{}",
                hex::encode(proposal.last_finalized_block_hash)
            ),
            basefee_sharing_pctg,
            network: None,
            l1_network: None,
            prover: None,
            signal_slots: proposal
                .signal_slots
                .iter()
                .map(|s| format!("0x{}", hex::encode(s)))
                .collect(),
            sources,
            blobs,
            checkpoint: Some(RaikoCheckpoint {
                block_number: proposal.checkpoint.blockNumber.to::<u64>(),
                block_hash: format!("0x{}", hex::encode(proposal.checkpoint.blockHash)),
                state_root: format!("0x{}", hex::encode(proposal.checkpoint.stateRoot)),
            }),
            blob_proof_type: "proof_of_equivalence".to_string(),
        };

        if proof_request_bypass {
            let json = serde_json::to_string_pretty(&request)?;
            let raiko_url = format!("{}/v3/proof/batch/realtime", raiko_client.base_url);

            std::fs::write("/tmp/raiko_request.json", &json)?;

            let api_key_header = raiko_client
                .api_key
                .as_ref()
                .map(|k| format!("  -H 'X-API-KEY: {}' \\\n", k))
                .unwrap_or_default();
            let curl_script = format!(
                "#!/bin/bash\n\
                 # Generated by Catalyst — send this to your Raiko instance\n\
                 # Usage: RAIKO_URL=http://your-raiko:8080 bash /tmp/raiko_curl.sh\n\n\
                 RAIKO_URL=\"${{RAIKO_URL:-{raiko_url}}}\"\n\n\
                 curl -X POST \"$RAIKO_URL\" \\\n\
                 {api_key_header}\
                 \x20 -H 'Content-Type: application/json' \\\n\
                 \x20 -d @/tmp/raiko_request.json\n"
            );
            std::fs::write("/tmp/raiko_curl.sh", &curl_script)?;

            info!(
                "PROOF_REQUEST_BYPASS: Raiko request dumped.\n\
                 Request JSON: /tmp/raiko_request.json\n\
                 Curl script:  /tmp/raiko_curl.sh\n\
                 Raiko URL:    {}\n\
                 Skipping Raiko call and L1 submission.",
                raiko_url
            );

            return Ok(SubmissionResult {
                new_last_finalized_block_hash: proposal.checkpoint.blockHash,
            });
        }

        // Set user op status to ProvingBlock before requesting proof from Raiko
        if let Some(ref store) = status_store {
            for op in &proposal.user_ops {
                store.set(
                    op.id,
                    &UserOpStatus::ProvingBlock {
                        block_id: proposal.checkpoint.blockNumber.to::<u64>(),
                    },
                );
            }
            // Also track L2 direct UserOps
            for id in &proposal.l2_user_op_ids {
                store.set(
                    *id,
                    &UserOpStatus::ProvingBlock {
                        block_id: proposal.checkpoint.blockNumber.to::<u64>(),
                    },
                );
            }
        }

        let proof = raiko_client.get_proof(&request).await?;
        proposal.zk_proof = Some(proof);
    }

    // Step 2: Send L1 transaction
    let mut user_op_ids: Vec<u64> = proposal.user_ops.iter().map(|op| op.id).collect();
    user_op_ids.extend(&proposal.l2_user_op_ids);
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
        // Mark all user ops (L1 and L2) as rejected on failure
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
            for id in &proposal.l2_user_op_ids {
                store.set(
                    *id,
                    &UserOpStatus::Rejected {
                        reason: reason.clone(),
                    },
                );
            }
        }
        return Err(err);
    }

    // Step 3: After successful submission, the new lastFinalizedBlockHash is the checkpoint's blockHash
    let new_last_finalized_block_hash = proposal.checkpoint.blockHash;

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

            // Clean up status entries after 60s (client should have polled by then)
            let cleanup_store = store.clone();
            let cleanup_ids = user_op_ids.clone();
            tokio::spawn(async move {
                tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
                for id in &cleanup_ids {
                    cleanup_store.remove(*id);
                }
            });
        });
    }

    Ok(SubmissionResult {
        new_last_finalized_block_hash,
    })
}
