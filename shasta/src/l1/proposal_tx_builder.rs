use crate::l1::{
    bindings::{
        IInbox::ProposeInput, LibBlobs::BlobReference, Multicall, SurgeInbox, UserOpsSubmitter,
    },
    config::ContractAddresses,
};
use crate::l2::bindings::ICheckpointStore::Checkpoint;
use crate::node::proposal_manager::{
    bridge_handler::{L1Call, UserOpData},
    proposal::Proposal,
};
use crate::shared_abi::bindings::Bridge;
use alloy::{
    consensus::SidecarBuilder,
    eips::eip4844::BlobTransactionSidecar,
    primitives::{
        Address, Bytes, U256,
        aliases::{U24, U48},
    },
    providers::{DynProvider, Provider},
    rpc::types::TransactionRequest,
    signers::Signer,
    sol_types::SolValue,
};
use alloy_json_rpc::RpcError;
use anyhow::Error;
use common::l1::{fees_per_gas::FeesPerGas, tools, transaction_error::TransactionError};
use taiko_protocol::shasta::{
    BlobCoder,
    manifest::{BlockManifest, DerivationSourceManifest},
};
use tracing::warn;

pub struct ProposalTxBuilder {
    provider: DynProvider,
    extra_gas_percentage: u64,
    checkpoint_signer: alloy::signers::local::PrivateKeySigner,
}

impl ProposalTxBuilder {
    pub fn new(
        provider: DynProvider,
        extra_gas_percentage: u64,
        checkpoint_signer: alloy::signers::local::PrivateKeySigner,
    ) -> Self {
        Self {
            provider,
            extra_gas_percentage,
            checkpoint_signer,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn build_propose_tx(
        &self,
        batch: Proposal,
        from: Address,
        contract_addresses: ContractAddresses,
    ) -> Result<TransactionRequest, Error> {
        let tx_blob = self
            .build_propose_blob(batch, from, contract_addresses)
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
        batch: Proposal,
        from: Address,
        contract_addresses: ContractAddresses,
    ) -> Result<TransactionRequest, Error> {
        let mut multicalls: Vec<Multicall::Call> = vec![];

        // Add user op to multicall
        // Note: Only adding the first call, since more calls are not expected for the POC
        if !batch.user_ops.is_empty() {
            multicalls.push(self.build_user_op_call(batch.user_ops.first().unwrap().clone()));
        }

        // Add the proposal to the multicall
        // This must always follow the user ops
        multicalls.push(
            self.build_propose_call(&batch, contract_addresses.shasta_inbox)
                .await?,
        );

        // Add L1 calls initiated by L2 blocks in the proposal
        if !batch.l1_calls.is_empty() {
            multicalls.push(self.build_l1_call_call(
                batch.l1_calls.first().unwrap().clone(),
                contract_addresses.bridge,
            ));
        }

        // Build the multicall transaction request
        let multicall = Multicall::new(contract_addresses.proposer_multicall, &self.provider);
        let call = multicall.multicall(multicalls);

        let tx = TransactionRequest::default()
            .to(contract_addresses.proposer_multicall)
            .from(from)
            .input(call.calldata().clone().into());

        Ok(tx)
    }

    // Surge: builds the 161-byte proof data
    // [0..96: ABI-encoded checkpoint][96..161: signed checkpoint digest]
    async fn build_proof_data(&self, checkpoint: &Checkpoint) -> Result<Bytes, Error> {
        let checkpoint_encoded = checkpoint.abi_encode();
        let checkpoint_digest = alloy::primitives::keccak256(&checkpoint_encoded);
        let signature = self.checkpoint_signer.sign_hash(&checkpoint_digest).await?;

        let mut signature_bytes = [0_u8; 65];
        signature_bytes[..32].copy_from_slice(signature.r().to_be_bytes::<32>().as_slice());
        signature_bytes[32..64].copy_from_slice(signature.s().to_be_bytes::<32>().as_slice());
        signature_bytes[64] = (signature.v() as u8) + 27;

        let mut proof_data = Vec::with_capacity(161);
        proof_data.extend_from_slice(&checkpoint_encoded);
        proof_data.extend_from_slice(&signature_bytes);
        Ok(Bytes::from(proof_data))
    }

    // Surge: Multicall builders

    fn build_user_op_call(&self, user_op_data: UserOpData) -> Multicall::Call {
        let submitter = UserOpsSubmitter::new(user_op_data.user_op_submitter, &self.provider);
        let call =
            submitter.executeBatch(vec![user_op_data.user_op], user_op_data.user_op_signature);

        Multicall::Call {
            target: user_op_data.user_op_submitter,
            value: U256::ZERO,
            data: call.calldata().clone(),
        }
    }

    async fn build_propose_call(
        &self,
        batch: &Proposal,
        inbox_address: Address,
    ) -> Result<Multicall::Call, anyhow::Error> {
        let mut block_manifests = <Vec<BlockManifest>>::with_capacity(batch.l2_blocks.len());
        for l2_block in &batch.l2_blocks {
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
        let sidecar: BlobTransactionSidecar = sidecar_builder.build()?;

        // Build the propose input.
        let input = ProposeInput {
            deadline: U48::ZERO,
            blobReference: BlobReference {
                blobStartIndex: 0,
                numBlobs: sidecar.blobs.len().try_into()?,
                offset: U24::ZERO,
            },
            numForcedInclusions: u8::from(batch.num_forced_inclusion),
        };

        let inbox = SurgeInbox::new(inbox_address, self.provider.clone());
        let encoded_proposal_input = inbox.encodeProposeInput(input).call().await?;

        // Surge: using `proposeWithProof(..)` in Surge Inbox
        let proof_data = self.build_proof_data(&batch.checkpoint).await?;
        let call = inbox.proposeWithProof(Bytes::new(), encoded_proposal_input, proof_data);

        Ok(Multicall::Call {
            target: inbox_address,
            value: U256::ZERO,
            data: call.calldata().clone(),
        })
    }

    fn build_l1_call_call(&self, l1_call: L1Call, bridge_address: Address) -> Multicall::Call {
        let bridge = Bridge::new(bridge_address, &self.provider);
        let call = bridge.processMessage(l1_call.message_from_l2, l1_call.signal_slot_proof);

        Multicall::Call {
            target: bridge_address,
            value: U256::ZERO,
            data: call.calldata().clone(),
        }
    }
}
