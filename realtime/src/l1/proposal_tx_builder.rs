use crate::l1::{
    bindings::{
        BlobReference, Multicall, ProofType, ProposeInput, ProposeInputV2, RealTimeInbox, SubProof,
        MOCK_ECDSA_BIT_FLAG,
    },
    config::ContractAddresses,
};
use crate::node::proposal_manager::{
    bridge_handler::{L1Call, UserOp},
    proposal::Proposal,
};
use crate::shared_abi::bindings::Bridge;
use alloy::{
    consensus::SidecarBuilder,
    eips::eip7594::BlobTransactionSidecarEip7594,
    network::TransactionBuilder7594,
    primitives::{
        Address, Bytes, U256,
        aliases::{U24, U48},
    },
    providers::{DynProvider, Provider},
    rpc::types::TransactionRequest,
    sol_types::SolValue,
};
use anyhow::Error;
use common::l1::fees_per_gas::FeesPerGas;
use taiko_protocol::shasta::{
    BlobCoder,
    manifest::{BlockManifest, DerivationSourceManifest},
};
use tracing::{info, warn};

pub struct ProposalTxBuilder {
    provider: DynProvider,
    extra_gas_percentage: u64,
    proof_type: ProofType,
    mock_mode: bool,
}

impl ProposalTxBuilder {
    pub fn new(
        provider: DynProvider,
        extra_gas_percentage: u64,
        proof_type: ProofType,
        mock_mode: bool,
    ) -> Self {
        Self {
            provider,
            extra_gas_percentage,
            proof_type,
            mock_mode,
        }
    }

    /// Gas estimation is skipped for blob transactions because `eth_estimateGas`
    /// cannot simulate blobs — the `BLOBHASH` opcode returns zero during estimation,
    /// causing spurious reverts that mask the real outcome. Instead we use a fixed
    /// gas limit and rely on the `TransactionMonitor`'s receipt check: if the on-chain
    /// execution reverts, the monitor sends `TransactionError::TransactionReverted`
    /// through the error channel, and the node's main loop triggers
    /// `recover_from_failed_submission` (reorg back to last finalized head).
    const BLOB_TX_GAS_LIMIT: u64 = 3_000_000;

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

        let tx_blob_gas = Self::BLOB_TX_GAS_LIMIT
            + Self::BLOB_TX_GAS_LIMIT * self.extra_gas_percentage / 100;

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
        // Collect required return signals from all l1_calls that expect an L1→L2
        // return signal to be produced by their invoked target. When non-empty, the
        // multicall is structured as:
        //   [tentativePropose, user_ops..., l1_calls..., finalizePropose]
        // so that processMessage runs against the tentative state root, its invoked
        // L1 callback produces the required return signal via Bridge.sendMessage,
        // and finalizePropose verifies those signals at the end.
        let required_return_signals: Vec<alloy::primitives::FixedBytes<32>> = batch
            .l1_calls
            .iter()
            .filter_map(|c| c.required_return_signal)
            .collect();

        let use_deferred = !required_return_signals.is_empty();

        // Build the inbox call(s) + blob sidecar. Returns either a single
        // `propose` call (classic flow) or a pair of (tentative, finalize) calls.
        let (inbox_calls, blob_sidecar) = self
            .build_inbox_calls(
                &batch,
                contract_addresses.realtime_inbox,
                use_deferred,
                &required_return_signals,
            )
            .await?;

        // If no user ops and no L1 calls and no deferred flow, go direct.
        if batch.user_ops.is_empty() && batch.l1_calls.is_empty() {
            if inbox_calls.len() == 1 {
                info!("Sending proposal directly to RealTimeInbox (no multicall)");
                let tx = TransactionRequest::default()
                    .to(contract_addresses.realtime_inbox)
                    .from(from)
                    .input(inbox_calls.into_iter().next().unwrap().data.into())
                    .with_blob_sidecar(blob_sidecar);
                return Ok(tx);
            }
            // Otherwise fall through to multicall assembly
        }

        let mut multicalls: Vec<Multicall::Call> = vec![];

        if use_deferred {
            // Deferred flow: [user_ops..., tentativePropose, l1_calls..., finalizePropose]
            //
            // User ops must run before tentativePropose because L1 UserOps are what
            // emit the existingSignals that tentativePropose verifies. Ordering them
            // after would leave those signals unsent and tentativePropose would revert.

            // 1. User ops (emit existingSignals on L1)
            for user_op in &batch.user_ops {
                let user_op_call = self.build_user_op_call(user_op.clone());
                info!("Added user op to Multicall: {:?}", &user_op_call);
                multicalls.push(user_op_call);
            }

            // 2. tentativePropose (inbox_calls[0]) — verifies existingSignals now present
            info!("Added tentativePropose to Multicall: {:?}", &inbox_calls[0]);
            multicalls.push(inbox_calls[0].clone());

            // 3. L1 calls (processMessage for L2→L1 signals — each triggers its
            //    target's L1 callback which produces an L1→L2 return signal)
            for l1_call in &batch.l1_calls {
                let l1_call_call =
                    self.build_l1_call_call(l1_call.clone(), contract_addresses.bridge);
                info!("Added L1 call to Multicall: {:?}", &l1_call_call);
                multicalls.push(l1_call_call);
            }

            // 4. finalizePropose (inbox_calls[1]) — verifies requiredReturnSignals
            info!("Added finalizePropose to Multicall: {:?}", &inbox_calls[1]);
            multicalls.push(inbox_calls[1].clone());
        } else {
            // Classic flow: [user_ops..., propose, l1_calls...]
            for user_op in &batch.user_ops {
                let user_op_call = self.build_user_op_call(user_op.clone());
                info!("Added user op to Multicall: {:?}", &user_op_call);
                multicalls.push(user_op_call);
            }

            info!("Added proposal to Multicall: {:?}", &inbox_calls[0]);
            multicalls.push(inbox_calls[0].clone());

            for l1_call in &batch.l1_calls {
                let l1_call_call =
                    self.build_l1_call_call(l1_call.clone(), contract_addresses.bridge);
                info!("Added L1 call to Multicall: {:?}", &l1_call_call);
                multicalls.push(l1_call_call);
            }
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

    /// Build the inbox call(s) + blob sidecar.
    ///
    /// When `use_deferred` is false, returns `[propose_call]` — the classic single
    /// atomic propose path.
    ///
    /// When `use_deferred` is true, returns `[tentativePropose_call, finalizePropose_call]`.
    /// `batch.signal_slots` is split into `existing_signals` (signals already on L1
    /// at proposal time, verified by tentativePropose) and `required_return_signals`
    /// (signals produced later in the multicall by L1 callbacks, verified by
    /// finalizePropose). The ZK proof commits to the union hash.
    async fn build_inbox_calls(
        &self,
        batch: &Proposal,
        inbox_address: Address,
        use_deferred: bool,
        required_return_signals: &[alloy::primitives::FixedBytes<32>],
    ) -> Result<(Vec<Multicall::Call>, BlobTransactionSidecarEip7594), anyhow::Error> {
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
        let sidecar: BlobTransactionSidecarEip7594 = sidecar_builder.build_7594()?;

        let inbox = RealTimeInbox::new(inbox_address, self.provider.clone());

        // Encode the raw proof as SubProof[] for the SurgeVerifier
        let raw_proof = batch
            .zk_proof
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("ZK proof not set on proposal"))?
            .clone();

        let bit_flag = if self.mock_mode {
            MOCK_ECDSA_BIT_FLAG
        } else {
            self.proof_type.proof_bit_flag()
        };
        let sub_proofs = vec![SubProof {
            proofBitFlag: bit_flag,
            data: Bytes::from(raw_proof),
        }];
        let proof = Bytes::from(sub_proofs.abi_encode());

        let blob_reference = BlobReference {
            blobStartIndex: 0,
            numBlobs: sidecar.blobs.len().try_into()?,
            offset: U24::ZERO,
        };

        // Convert L1 Checkpoint type for the inbox call
        let checkpoint = crate::l1::bindings::ICheckpointStore::Checkpoint {
            blockNumber: batch.checkpoint.blockNumber,
            blockHash: batch.checkpoint.blockHash,
            stateRoot: batch.checkpoint.stateRoot,
        };

        if !use_deferred {
            // Classic propose flow
            let propose_input = ProposeInput {
                blobReference: blob_reference,
                signalSlots: batch.signal_slots.clone(),
                maxAnchorBlockNumber: U48::from(batch.max_anchor_block_number),
            };
            let encoded_input = Bytes::from(propose_input.abi_encode());
            let call = inbox.propose(encoded_input, checkpoint, proof);

            return Ok((
                vec![Multicall::Call {
                    target: inbox_address,
                    value: U256::ZERO,
                    data: call.calldata().clone(),
                }],
                sidecar,
            ));
        }

        // Deferred propose flow — split signal slots.
        // `batch.signal_slots` should carry the UNION of existing and required-return
        // slots (the anchor on L2 consumes the union as fast signals). We derive
        // `existing_signals` by subtracting the required-return list from the union.
        let required_set: std::collections::HashSet<_> =
            required_return_signals.iter().copied().collect();
        let existing_signals: Vec<alloy::primitives::FixedBytes<32>> = batch
            .signal_slots
            .iter()
            .copied()
            .filter(|s| !required_set.contains(s))
            .collect();

        let propose_input_v2 = ProposeInputV2 {
            blobReference: blob_reference,
            existingSignals: existing_signals,
            requiredReturnSignals: required_return_signals.to_vec(),
            maxAnchorBlockNumber: U48::from(batch.max_anchor_block_number),
        };
        let encoded_input = Bytes::from(propose_input_v2.abi_encode());

        let tentative_call = inbox.tentativePropose(encoded_input, checkpoint, proof);
        let finalize_call = inbox.finalizePropose(required_return_signals.to_vec());

        Ok((
            vec![
                Multicall::Call {
                    target: inbox_address,
                    value: U256::ZERO,
                    data: tentative_call.calldata().clone(),
                },
                Multicall::Call {
                    target: inbox_address,
                    value: U256::ZERO,
                    data: finalize_call.calldata().clone(),
                },
            ],
            sidecar,
        ))
    }

    fn build_l1_call_call(&self, l1_call: L1Call, bridge_address: Address) -> Multicall::Call {
        let bridge = Bridge::new(bridge_address, &self.provider);
        let call = bridge.processMessage(l1_call.message_from_l2.clone(), l1_call.signal_slot_proof);

        Multicall::Call {
            target: bridge_address,
            value: U256::ZERO,
            data: call.calldata().clone(),
        }
    }
}
