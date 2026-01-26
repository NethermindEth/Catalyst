use alloy::{
    consensus::SidecarBuilder,
    network::{TransactionBuilder, TransactionBuilder7594},
    primitives::{
        Address, Bytes,
        aliases::{U24, U48},
    },
    providers::{DynProvider, Provider},
    rpc::types::TransactionRequest,
};
use alloy_json_rpc::RpcError;
use anyhow::Error;
use common::l1::{fees_per_gas::FeesPerGas, tools, transaction_error::TransactionError};
use common::shared::l2_block_v2::L2BlockV2;
use taiko_bindings::inbox::{IInbox::ProposeInput, Inbox, LibBlobs::BlobReference};
use taiko_protocol::shasta::{
    BlobCoder,
    manifest::{BlockManifest, DerivationSourceManifest},
};
use tracing::warn;

pub struct ProposalTxBuilder {
    provider: DynProvider,
    extra_gas_percentage: u64,
}

impl ProposalTxBuilder {
    pub fn new(provider: DynProvider, extra_gas_percentage: u64) -> Self {
        Self {
            provider,
            extra_gas_percentage,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn build_propose_tx(
        &self,
        l2_blocks: Vec<L2BlockV2>,
        from: Address,
        to: Address,
        num_forced_inclusion: u16,
    ) -> Result<TransactionRequest, Error> {
        let tx_blob = self
            .build_propose_blob(l2_blocks, from, to, num_forced_inclusion)
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

        // Update gas params for eip4844 transaction
        let tx_blob = fees_per_gas.update_eip4844(tx_blob, tx_blob_gas);

        Ok(tx_blob)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn build_propose_blob(
        &self,
        l2_blocks: Vec<L2BlockV2>,
        from: Address,
        to: Address,
        num_forced_inclusion: u16,
    ) -> Result<TransactionRequest, Error> {
        let mut block_manifests = <Vec<BlockManifest>>::with_capacity(l2_blocks.len());
        for l2_block in &l2_blocks {
            // Build the block manifests.
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

        // Build the proposal manifest.
        let manifest = DerivationSourceManifest {
            blocks: block_manifests,
        };

        let manifest_data = manifest
            .encode_and_compress()
            .map_err(|e| Error::msg(format!("Can't encode and compress manifest: {e}")))?;

        let sidecar_builder: SidecarBuilder<BlobCoder> = SidecarBuilder::from_slice(&manifest_data);
        let sidecar = sidecar_builder.build_7594()?;

        // Build the propose input.
        let input = ProposeInput {
            deadline: U48::ZERO,
            blobReference: BlobReference {
                blobStartIndex: 0,
                numBlobs: sidecar.blobs.len().try_into()?,
                offset: U24::ZERO,
            },
            numForcedInclusions: num_forced_inclusion,
        };

        let inbox = Inbox::new(to, self.provider.clone());
        let encoded_proposal_input = inbox.encodeProposeInput(input).call().await?;

        let tx = TransactionRequest::default()
            .with_from(from)
            .with_to(to)
            .with_blob_sidecar(sidecar)
            .with_call(&Inbox::proposeCall {
                _lookahead: Bytes::new(),
                _data: encoded_proposal_input,
            });

        Ok(tx)
    }
}
