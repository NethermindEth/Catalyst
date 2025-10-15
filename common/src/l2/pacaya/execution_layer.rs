use super::super::bindings::{TaikoAnchor, TaikoAnchor::BaseFeeConfig};
use super::super::config::{GOLDEN_TOUCH_ADDRESS, GOLDEN_TOUCH_PRIVATE_KEY};
use alloy::{
    consensus::{SignableTransaction, TxEnvelope, transaction::Recovered},
    primitives::{Address, B256},
    providers::{DynProvider, Provider},
    rpc::types::Transaction,
    signers::Signature,
};
use anyhow::Error;
use tracing::debug;

pub struct ExecutionLayer {
    provider: DynProvider,
    taiko_anchor: TaikoAnchor::TaikoAnchorInstance<DynProvider>,
    chain_id: u64,
}

impl ExecutionLayer {
    pub fn new(provider: DynProvider, taiko_anchor_address: Address, chain_id: u64) -> Self {
        let taiko_anchor = TaikoAnchor::new(taiko_anchor_address, provider.clone());

        Self {
            provider,
            taiko_anchor,
            chain_id,
        }
    }

    pub async fn construct_anchor_tx(
        &self,
        parent_hash: B256,
        anchor_block_id: u64,
        anchor_state_root: B256,
        parent_gas_used: u32,
        base_fee_config: BaseFeeConfig,
        base_fee: u64,
    ) -> Result<Transaction, Error> {
        // Create the contract call
        let nonce = self
            .provider
            .get_transaction_count(GOLDEN_TOUCH_ADDRESS)
            .block_id(parent_hash.into())
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get nonce: {}", e))?;
        let call_builder = self
            .taiko_anchor
            .anchorV3(
                anchor_block_id,
                anchor_state_root,
                parent_gas_used,
                base_fee_config,
                vec![],
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
        crate::crypto::fixed_k_signer::sign_hash_deterministic(GOLDEN_TOUCH_PRIVATE_KEY, hash)
    }
}
