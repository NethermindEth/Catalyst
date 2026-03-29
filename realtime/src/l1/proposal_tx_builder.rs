use crate::l1::{
    bindings::{BlobReference, Multicall, ProofType, ProposeInput, RealTimeInbox, SubProof},
    config::ContractAddresses,
};
use crate::node::proposal_manager::{
    bridge_handler::{L1Call, UserOp},
    proposal::Proposal,
};
use crate::shared_abi::bindings::Bridge;
use alloy::{
    consensus::SidecarBuilder,
    eips::eip4844::BlobTransactionSidecar,
    network::TransactionBuilder4844,
    primitives::{
        Address, Bytes, U256,
        aliases::{U24, U48},
    },
    providers::{DynProvider, Provider},
    rpc::types::TransactionRequest,
    sol_types::SolValue,
};
use alloy_json_rpc::RpcError;
use anyhow::Error;
use common::l1::{fees_per_gas::FeesPerGas, tools, transaction_error::TransactionError};
use taiko_protocol::shasta::{
    BlobCoder,
    manifest::{BlockManifest, DerivationSourceManifest},
};
use tracing::{info, warn};

pub struct ProposalTxBuilder {
    provider: DynProvider,
    extra_gas_percentage: u64,
    proof_type: ProofType,
}

impl ProposalTxBuilder {
    pub fn new(provider: DynProvider, extra_gas_percentage: u64, proof_type: ProofType) -> Self {
        Self {
            provider,
            extra_gas_percentage,
            proof_type,
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
                    "Build proposeBatch: Failed to estimate gas for blob transaction: {}. Force-sending with 500000 gas.",
                    e
                );
                500_000
            }
        };
        let tx_blob_gas = tx_blob_gas + tx_blob_gas * self.extra_gas_percentage / 100;

        let fees_per_gas = match FeesPerGas::get_fees_per_gas(&self.provider).await {
            Ok(fees_per_gas) => fees_per_gas,
            Err(e) => {
                warn!("Build proposeBatch: Failed to get fees per gas: {}", e);
                return Ok(tx_blob);
            }
        };

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
        if !batch.user_ops.is_empty()
            && let Some(user_op) = batch.user_ops.first()
        {
            let user_op_call = self.build_user_op_call(user_op.clone());
            info!("Added user op to Multicall: {:?}", &user_op_call);
            multicalls.push(user_op_call);
        }

        // Build the propose call and blob sidecar
        let (propose_call, blob_sidecar) = self
            .build_propose_call(&batch, contract_addresses.realtime_inbox)
            .await?;

        // If no user ops or L1 calls, send directly to inbox (skip multicall)
        if batch.user_ops.is_empty() && batch.l1_calls.is_empty() {
            info!("Sending proposal directly to RealTimeInbox (no multicall)");
            let tx = TransactionRequest::default()
                .to(contract_addresses.realtime_inbox)
                .from(from)
                .input(propose_call.data.into())
                .with_blob_sidecar(blob_sidecar);
            return Ok(tx);
        }

        info!("Added proposal to Multicall: {:?}", &propose_call);
        multicalls.push(propose_call.clone());

        // Add L1 calls
        if !batch.l1_calls.is_empty()
            && let Some(l1_call) = batch.l1_calls.first()
        {
            let l1_call_call = self.build_l1_call_call(l1_call.clone(), contract_addresses.bridge);
            info!("Added L1 call to Multicall: {:?}", &l1_call_call);
            multicalls.push(l1_call_call.clone());
        }

        let multicall = Multicall::new(contract_addresses.proposer_multicall, &self.provider);
        let call = multicall.multicall(multicalls);

        let tx = TransactionRequest::default()
            .to(contract_addresses.proposer_multicall)
            .from(from)
            .input(call.calldata().clone().into())
            .with_blob_sidecar(blob_sidecar);

        Ok(tx)
    }

    fn build_user_op_call(&self, user_op_data: UserOp) -> Multicall::Call {
        Multicall::Call {
            target: user_op_data.submitter,
            value: U256::ZERO,
            data: user_op_data.calldata,
        }
    }

    async fn build_propose_call(
        &self,
        batch: &Proposal,
        inbox_address: Address,
    ) -> Result<(Multicall::Call, BlobTransactionSidecar), anyhow::Error> {
        let mut block_manifests = <Vec<BlockManifest>>::with_capacity(batch.l2_blocks.len());
        for l2_block in &batch.l2_blocks {
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

        let manifest_data = manifest
            .encode_and_compress()
            .map_err(|e| Error::msg(format!("Can't encode and compress manifest: {e}")))?;

        let sidecar_builder: SidecarBuilder<BlobCoder> = SidecarBuilder::from_slice(&manifest_data);
        let sidecar: BlobTransactionSidecar = sidecar_builder.build()?;

        let inbox = RealTimeInbox::new(inbox_address, self.provider.clone());

        // Encode the raw proof as SubProof[] for the SurgeVerifier
        let raw_proof = batch
            .zk_proof
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("ZK proof not set on proposal"))?
            .clone();

        let sub_proofs = vec![SubProof {
            proofBitFlag: self.proof_type.proof_bit_flag(),
            data: Bytes::from(raw_proof),
        }];
        let proof = Bytes::from(sub_proofs.abi_encode());

        // Build ProposeInput and ABI-encode it as the _data parameter
        let blob_reference = BlobReference {
            blobStartIndex: 0,
            numBlobs: sidecar.blobs.len().try_into()?,
            offset: U24::ZERO,
        };

        let propose_input = ProposeInput {
            blobReference: blob_reference,
            signalSlots: batch.signal_slots.clone(),
            maxAnchorBlockNumber: U48::from(batch.max_anchor_block_number),
        };

        let encoded_input = Bytes::from(propose_input.abi_encode());

        // Convert L1 Checkpoint type for the propose call
        let checkpoint = crate::l1::bindings::ICheckpointStore::Checkpoint {
            blockNumber: batch.checkpoint.blockNumber,
            blockHash: batch.checkpoint.blockHash,
            stateRoot: batch.checkpoint.stateRoot,
        };

        let call = inbox.propose(encoded_input, checkpoint, proof);

        Ok((
            Multicall::Call {
                target: inbox_address,
                value: U256::ZERO,
                data: call.calldata().clone(),
            },
            sidecar,
        ))
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
