// TODO remove allow dead_code when the module is used
#![allow(dead_code)]
use super::bindings::Anchor;
use alloy::{
    consensus::{SignableTransaction, TxEnvelope, transaction::Recovered},
    primitives::{Address, B256, Bytes, FixedBytes, Uint},
    providers::{DynProvider, Provider},
    rpc::types::Transaction,
    signers::Signature,
};
use anyhow::Error;
use common::crypto::{GOLDEN_TOUCH_ADDRESS, GOLDEN_TOUCH_PRIVATE_KEY};
use common::shared::{alloy_tools, execution_layer::ExecutionLayer as ExecutionLayerCommon};
use pacaya::l2::config::TaikoConfig;
use taiko_event_indexer::interface::ShastaProposeInput;
use tracing::{debug, info};

pub struct L2ExecutionLayer {
    common: ExecutionLayerCommon,
    provider: DynProvider,
    shasta_anchor: Anchor::AnchorInstance<DynProvider>,
    chain_id: u64,
    config: TaikoConfig,
}

impl L2ExecutionLayer {
    pub async fn new(taiko_config: TaikoConfig) -> Result<Self, Error> {
        let provider =
            alloy_tools::create_alloy_provider_without_wallet(&taiko_config.taiko_geth_url).await?;

        let chain_id = provider
            .get_chain_id()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get chain ID: {}", e))?;
        info!("L2 Chain ID: {}", chain_id);

        let shasta_anchor = Anchor::new(taiko_config.taiko_anchor_address, provider.clone());

        let common = ExecutionLayerCommon::new(provider.clone()).await?;

        Ok(Self {
            common,
            provider,
            shasta_anchor,
            chain_id,
            config: taiko_config,
        })
    }

    pub fn common(&self) -> &ExecutionLayerCommon {
        &self.common
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn construct_anchor_tx(
        &self,
        preconfer_address: &Address,
        l2_block_number: u16,
        parent_hash: B256,
        anchor_block_id: u64,
        anchor_block_hash: B256,
        anchor_state_root: B256,
        base_fee: u64,
        propose_input: ShastaProposeInput,
    ) -> Result<Transaction, Error> {
        debug!(
            "Constructing anchor transaction for block number: {}",
            l2_block_number
        );
        let nonce = self
            .provider
            .get_transaction_count(GOLDEN_TOUCH_ADDRESS)
            .block_id(parent_hash.into())
            .await
            .map_err(|e| {
                self.common
                    .chain_error("Failed to get transaction count", Some(&e.to_string()))
            })?;

        let call_builder = self
            .shasta_anchor
            .anchorV4(
                Anchor::ProposalParams {
                    proposalId: propose_input.core_state.nextProposalId,
                    proposer: preconfer_address.clone(),
                    proverAuth: Bytes::new(), // no prover designation for now
                    bondInstructionsHash: FixedBytes::from([0u8; 32]),
                    bondInstructions: vec![],
                },
                Anchor::BlockParams {
                    blockIndex: l2_block_number,
                    anchorBlockNumber: Uint::<48, 1>::from(anchor_block_id),
                    anchorBlockHash: anchor_block_hash,
                    anchorStateRoot: anchor_state_root,
                },
            )
            .gas(1_000_000) // value expected by Taiko
            .max_fee_per_gas(u128::from(base_fee)) // value expected by Taiko
            .max_priority_fee_per_gas(0) // value expected by Taiko
            .nonce(nonce)
            .chain_id(self.chain_id);

        let typed_tx = call_builder
            .into_transaction_request()
            .build_typed_tx()
            .map_err(|_| anyhow::anyhow!("AnchorTX: Failed to build typed transaction"))?;

        let tx_eip1559 = typed_tx
            .eip1559()
            .ok_or_else(|| anyhow::anyhow!("AnchorTX: Failed to extract EIP-1559 transaction"))?;

        let signature = self.sign_hash_deterministic(tx_eip1559.signature_hash())?;
        let sig_tx = tx_eip1559.clone().into_signed(signature);

        let tx_envelope = TxEnvelope::from(sig_tx);

        debug!("AnchorTX transaction hash: {}", tx_envelope.tx_hash());

        let tx = Transaction {
            inner: Recovered::new_unchecked(tx_envelope, GOLDEN_TOUCH_ADDRESS),
            block_hash: None,
            block_number: None,
            transaction_index: None,
            effective_gas_price: None,
        };
        Ok(tx)
    }

    fn sign_hash_deterministic(&self, hash: B256) -> Result<Signature, Error> {
        common::crypto::fixed_k_signer::sign_hash_deterministic(GOLDEN_TOUCH_PRIVATE_KEY, hash)
    }
}
