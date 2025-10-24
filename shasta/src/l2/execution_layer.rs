// TODO remove allow dead_code when the module is used
#![allow(dead_code)]
use super::bindings::ShastaAnchor;
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
use tracing::{debug, info};

pub struct L2ExecutionLayer {
    common: ExecutionLayerCommon,
    provider: DynProvider,
    shasta_anchor: ShastaAnchor::ShastaAnchorInstance<DynProvider>,
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

        let shasta_anchor = ShastaAnchor::new(taiko_config.taiko_anchor_address, provider.clone());

        let common = ExecutionLayerCommon::new(provider.clone()).await?;

        Ok(Self {
            common,
            provider,
            shasta_anchor,
            chain_id,
            config: taiko_config,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn construct_anchor_tx(
        &self,
        // proposal_id: u64,    // TODO: implement
        proposer: Address,
        l2_block_number: u16,
        parent_hash: B256,
        anchor_block_id: u64,
        // anchor_block_hash: B256,
        anchor_state_root: B256,
        base_fee: u64,
    ) -> Result<Transaction, Error> {
        let nonce = self
            .provider
            .get_transaction_count(GOLDEN_TOUCH_ADDRESS)
            .block_id(parent_hash.into())
            .await?;
        let call_builder = self
            .shasta_anchor
            .updateState(
                Uint::<48, 1>::from(0), // proposal_id
                proposer,
                Bytes::new(),                // no prover designation
                FixedBytes::from([0u8; 32]), // bond_instructions_hash, take them from the indexer
                vec![],
                l2_block_number,
                Uint::<48, 1>::from(anchor_block_id),
                B256::ZERO, // anchor_block_hash,
                anchor_state_root,
                Uint::<48, 1>::from(0), // 0 for whitelist
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
