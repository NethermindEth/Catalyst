//TODO: allow unused code until ProposalBuilder is used
#![allow(unused)]

use super::event_indexer::EventIndexer;
use alloy::{
    network::{TransactionBuilder, TransactionBuilder4844},
    primitives::{
        Address, Bytes,
        aliases::{U24, U48},
    },
    providers::{DynProvider, Provider},
    rpc::types::TransactionRequest,
};
use anyhow::Error;
use common::shared::l2_block::L2Block;
use std::sync::Arc;
use taiko_bindings::codec_optimized::{
    CodecOptimized::CodecOptimizedInstance, IInbox::ProposeInput, LibBlobs::BlobReference,
};
use taiko_bindings::i_inbox::IInbox;

use taiko_protocol::shasta::manifest::{BlockManifest, DerivationSourceManifest, ProposalManifest};

use alloy_json_rpc::RpcError;
use common::l1::{fees_per_gas::FeesPerGas, tools, transaction_error::TransactionError};
use tracing::{info, warn};

pub struct ProposalTxBuilder {
    provider: DynProvider,
    codec_address: Address,
    extra_gas_percentage: u64,
}

impl ProposalTxBuilder {
    pub fn new(provider: DynProvider, codec_address: Address, extra_gas_percentage: u64) -> Self {
        Self {
            provider,
            codec_address,
            extra_gas_percentage,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn build_propose_tx(
        &self,
        l2_blocks: Vec<L2Block>,
        anchor_block_number: u64,
        coinbase: Address,
        from: Address,
        to: Address,
        prover_auth_bytes: Bytes,
        event_indexer: Arc<EventIndexer>,
        num_forced_inclusion: u8,
    ) -> Result<TransactionRequest, Error> {
        let tx_blob = self
            .build_propose_blob(
                l2_blocks,
                anchor_block_number,
                coinbase,
                from,
                to,
                prover_auth_bytes,
                event_indexer,
                num_forced_inclusion,
            )
            .await?;
        let tx_blob_gas = match self.provider.estimate_gas(tx_blob.clone()).await {
            Ok(gas) => gas,
            Err(e) => {
                warn!(
                    "Build proposeBatch: Failed to estimate gas for blob transaction: {}",
                    e
                );
                match e {
                    RpcError::ErrorResp(err) => {
                        return Err(anyhow::anyhow!(
                            tools::convert_error_payload(&err.to_string())
                                .unwrap_or(TransactionError::EstimationFailed)
                        ));
                    }
                    _ => return Ok(tx_blob),
                }
            }
        };
        let tx_blob_gas = tx_blob_gas + tx_blob_gas * self.extra_gas_percentage / 100;

        // Get fees from the network
        let fees_per_gas = match FeesPerGas::get_fees_per_gas(&self.provider).await {
            Ok(fees_per_gas) => fees_per_gas,
            Err(e) => {
                warn!("Build proposeBatch: Failed to get fees per gas: {}", e);
                // In case of error return eip4844 transaction
                return Ok(tx_blob);
            }
        };

        // Get blob count
        let blob_count = tx_blob
            .sidecar
            .as_ref()
            .map_or(0, |sidecar| sidecar.blobs.len() as u64);

        // Calculate the cost of the eip4844 transaction
        let eip4844_cost = fees_per_gas.get_eip4844_cost(blob_count, tx_blob_gas).await;

        // Update gas params for eip4844 transaction
        let tx_blob = fees_per_gas.update_eip4844(tx_blob, tx_blob_gas);

        Ok(tx_blob)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn build_propose_blob(
        &self,
        l2_blocks: Vec<L2Block>,
        anchor_block_number: u64,
        coinbase: Address,
        from: Address,
        to: Address,
        prover_auth_bytes: Bytes,
        event_indexer: Arc<EventIndexer>,
        num_forced_inclusion: u8,
    ) -> Result<TransactionRequest, Error> {
        // Read cached propose input params from the event indexer.
        let cached_input_params = event_indexer
            .get_propose_input()
            .ok_or(Error::msg("Can't read shasta propose input"))?;
        info!(
            core_state = ?cached_input_params.core_state,
            proposals_count = cached_input_params.proposals.len(),
            transition_records_count = cached_input_params.transition_records.len(),
            checkpoint = ?cached_input_params.checkpoint,
            "cached propose input params"
        );

        let mut block_manifests = <Vec<BlockManifest>>::with_capacity(l2_blocks.len());
        for l2_block in &l2_blocks {
            // Build the block manifests.
            block_manifests.push(BlockManifest {
                timestamp: l2_block.timestamp_sec,
                coinbase,
                anchor_block_number,
                gas_limit: 0, /* Use 0 for gas limit as it will be set as its parent's gas
                               * limit during derivation. */
                transactions: l2_block
                    .prebuilt_tx_list
                    .tx_list
                    .iter()
                    .map(|tx| tx.clone().into())
                    .collect(),
            });
        }

        // Build the proposal manifest.
        let manifest = ProposalManifest {
            prover_auth_bytes,
            sources: vec![DerivationSourceManifest {
                blocks: block_manifests,
            }],
        };

        let manifest_data = manifest
            .encode_and_compress()
            .map_err(|e| Error::msg(format!("Can't encode and compress manifest: {e}")))?;

        let sidecar = common::blob::build_blob_sidecar(&manifest_data)?;

        // Build the propose input.
        let input = ProposeInput {
            deadline: U48::ZERO,
            coreState: cached_input_params.core_state,
            parentProposals: cached_input_params.proposals,
            blobReference: BlobReference {
                blobStartIndex: 0,
                numBlobs: sidecar.blobs.len().try_into()?,
                offset: U24::ZERO,
            },
            transitionRecords: cached_input_params.transition_records,
            checkpoint: cached_input_params.checkpoint,
            numForcedInclusions: num_forced_inclusion,
        };

        let codec = CodecOptimizedInstance::new(self.codec_address, self.provider.clone());
        let encoded_proposal_input = codec.encodeProposeInput(input).call().await?;

        let tx = TransactionRequest::default()
            .with_from(from)
            .with_to(to)
            .with_blob_sidecar(sidecar)
            .with_call(&IInbox::proposeCall {
                _lookahead: Bytes::new(),
                _data: Bytes::new(),
            });

        Ok(tx)
    }
}
