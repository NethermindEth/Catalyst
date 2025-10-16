use super::config::{GOLDEN_TOUCH_ADDRESS, GOLDEN_TOUCH_PRIVATE_KEY};
use super::{
    bindings::{Bridge, TaikoAnchor, TaikoAnchor::BaseFeeConfig},
    config::TaikoConfig,
};
use crate::shared::{alloy_tools, l2_slot_info::L2SlotInfo};
use alloy::{
    consensus::Transaction as AnchorTransaction,
    consensus::{SignableTransaction, TxEnvelope, transaction::Recovered},
    eips::BlockNumberOrTag,
    network::ReceiptResponse,
    primitives::{Address, B256, Bytes, U256, Uint},
    providers::{DynProvider, Provider},
    rpc::types::{Block as RpcBlock, Transaction},
    signers::Signature,
};
use anyhow::Error;
use serde_json::Value;
use std::time::Duration;
use tracing::{debug, info, warn};

pub struct L2ExecutionLayer {
    provider: DynProvider,
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

        let taiko_anchor = TaikoAnchor::new(taiko_config.taiko_anchor_address, provider.clone());

        Ok(Self {
            provider,
            taiko_anchor,
            chain_id,
            config: taiko_config,
        })
    }

    pub async fn get_l2_block_hash(&self, number: u64) -> Result<B256, Error> {
        let block = self
            .get_l2_block_header(BlockNumberOrTag::Number(number))
            .await?;
        Ok(block.header.hash)
    }

    pub async fn get_l2_block_header(&self, block: BlockNumberOrTag) -> Result<RpcBlock, Error> {
        self.provider
            .get_block_by_number(block)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get L2 block header: {}", e))?
            .ok_or(anyhow::anyhow!("Failed to get L2 block header"))
    }

    async fn get_latest_l2_block_with_txs(&self) -> Result<RpcBlock, Error> {
        self.provider
            .get_block_by_number(BlockNumberOrTag::Latest)
            .full()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get latest L2 block: {}", e))?
            .ok_or(anyhow::anyhow!("Failed to get latest L2 block"))
    }

    pub async fn get_balance(&self, address: Address) -> Result<U256, Error> {
        self.provider
            .get_balance(address)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get L2 balance: {}", e))
    }

    pub async fn get_forced_inclusion_form_l1origin(&self, block_id: u64) -> Result<bool, Error> {
        self.provider
            .raw_request::<_, Value>(
                std::borrow::Cow::Borrowed("taiko_l1OriginByID"),
                vec![Value::String(block_id.to_string())],
            )
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get forced inclusion: {}", e))?
            .get("isForcedInclusion")
            .and_then(Value::as_bool)
            .ok_or_else(|| anyhow::anyhow!("Failed to parse isForcedInclusion"))
    }

    pub async fn get_latest_l2_block_id(&self) -> Result<u64, Error> {
        self.provider
            .get_block_number()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get latest L2 block number: {}", e))
    }

    pub async fn get_l2_block_by_number(
        &self,
        number: u64,
        full_txs: bool,
    ) -> Result<alloy::rpc::types::Block, Error> {
        let mut block_by_number = self
            .provider
            .get_block_by_number(BlockNumberOrTag::Number(number));

        if full_txs {
            block_by_number = block_by_number.full();
        }

        block_by_number
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get L2 block by number: {}", e))?
            .ok_or(anyhow::anyhow!(
                "Failed to get L2 block {}: value was None",
                number
            ))
    }

    pub async fn get_transaction_by_hash(
        &self,
        hash: B256,
    ) -> Result<alloy::rpc::types::Transaction, Error> {
        self.provider
            .get_transaction_by_hash(hash)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get L2 transaction by hash: {}", e))?
            .ok_or(anyhow::anyhow!(
                "Failed to get L2 transaction: value is None"
            ))
    }

    pub async fn get_base_fee(
        &self,
        parent_hash: B256,
        parent_gas_used: u32,
        base_fee_config: BaseFeeConfig,
        l2_slot_timestamp: u64,
    ) -> Result<u64, Error> {
        let base_fee = self
            .taiko_anchor
            .getBasefeeV2(parent_gas_used, l2_slot_timestamp, base_fee_config)
            .block(parent_hash.into())
            .call()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get base fee: {}", e))?
            .basefee_;

        base_fee
            .try_into()
            .map_err(|err| anyhow::anyhow!("Failed to convert base fee to u64: {}", err))
    }

    pub async fn get_last_synced_anchor_block_id_from_taiko_anchor(&self) -> Result<u64, Error> {
        match self.taiko_anchor.lastSyncedBlock().call().await {
            Ok(block_id) => Ok(block_id),
            Err(_) => self
                .taiko_anchor
                .lastCheckpoint()
                .call()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to get last synced block: {}", e)),
        }
    }

    pub async fn get_last_synced_anchor_block_id_from_geth(&self) -> Result<u64, Error> {
        let block = self.get_latest_l2_block_with_txs().await?;
        let (anchor_tx, _) = match block.transactions.as_transactions() {
            Some(txs) => txs
                .split_first()
                .ok_or_else(|| anyhow::anyhow!("Cannot get anchor transaction from block"))?,
            None => return Err(anyhow::anyhow!("No transactions in block")),
        };

        Self::decode_anchor_id_from_tx_data(anchor_tx.input())
    }

    pub fn decode_anchor_id_from_tx_data(data: &[u8]) -> Result<u64, Error> {
        let tx_data =
            <TaikoAnchor::anchorV3Call as alloy::sol_types::SolCall>::abi_decode_validate(data)?;
        Ok(tx_data._anchorBlockId)
    }

    pub async fn transfer_eth_from_l2_to_l1(
        &self,
        amount: u128,
        dest_chain_id: u64,
        preconfer_address: Address,
        bridge_relayer_fee: u64,
    ) -> Result<(), Error> {
        info!(
            "Transfer ETH from L2 to L1: srcChainId: {}, dstChainId: {}",
            self.chain_id, dest_chain_id
        );

        let (provider, _) = alloy_tools::construct_alloy_provider(
            &self.config.signer,
            &self.config.taiko_geth_url,
            Some(preconfer_address),
        )
        .await?;

        self.transfer_eth_from_l2_to_l1_with_provider(
            provider,
            amount,
            dest_chain_id,
            preconfer_address,
            bridge_relayer_fee,
        )
        .await?;

        Ok(())
    }

    async fn transfer_eth_from_l2_to_l1_with_provider(
        &self,
        provider: DynProvider,
        amount: u128,
        dest_chain_id: u64,
        preconfer_address: Address,
        bridge_relayer_fee: u64,
    ) -> Result<(), Error> {
        let contract = Bridge::new(self.config.taiko_bridge_address, provider.clone());
        let gas_limit = contract
            .getMessageMinGasLimit(Uint::<256, 4>::from(0))
            .call()
            .await?;
        debug!("Bridge message gas limit: {}", gas_limit);

        let message = Bridge::Message {
            id: 0,
            fee: bridge_relayer_fee,
            gasLimit: gas_limit + 1,
            from: preconfer_address,
            srcChainId: self.chain_id,
            srcOwner: preconfer_address,
            destChainId: dest_chain_id,
            destOwner: preconfer_address,
            to: preconfer_address,
            value: Uint::<256, 4>::from(amount),
            data: Bytes::new(),
        };

        let mut fees = provider.estimate_eip1559_fees().await?;
        const ONE_GWEI: u128 = 1000000000;
        if fees.max_priority_fee_per_gas < ONE_GWEI {
            fees.max_priority_fee_per_gas = ONE_GWEI;
            fees.max_fee_per_gas += ONE_GWEI;
        }
        debug!("Fees: {:?}", fees);
        let nonce = provider.get_transaction_count(preconfer_address).await?;

        let tx_send_message = contract
            .sendMessage(message)
            .value(Uint::<256, 4>::from(
                amount + u128::from(bridge_relayer_fee),
            ))
            .from(preconfer_address)
            .nonce(nonce)
            .chain_id(self.chain_id)
            .max_fee_per_gas(fees.max_fee_per_gas)
            .max_priority_fee_per_gas(fees.max_priority_fee_per_gas);

        let tx_request = tx_send_message.into_transaction_request();
        const GAS_LIMIT: u64 = 500000;
        let tx_request = tx_request.gas_limit(GAS_LIMIT);
        let pending_tx = provider.send_transaction(tx_request).await?;

        let tx_hash = *pending_tx.tx_hash();
        info!("Bridge sendMessage tx hash: {}", tx_hash);

        const RECEIPT_TIMEOUT: Duration = Duration::from_secs(100);
        let receipt = pending_tx
            .with_timeout(Some(RECEIPT_TIMEOUT))
            .get_receipt()
            .await?;

        if receipt.status() {
            let block_number = if let Some(block_number) = receipt.block_number() {
                block_number
            } else {
                warn!("Block number not found for transaction {}", tx_hash);
                0
            };

            info!(
                "🌁 Transaction {} confirmed in block {}",
                tx_hash, block_number
            );
        } else if let Some(block_number) = receipt.block_number() {
            return Err(anyhow::anyhow!(
                crate::shared::alloy_tools::check_for_revert_reason(
                    &provider,
                    tx_hash,
                    block_number
                )
                .await
            ));
        } else {
            return Err(anyhow::anyhow!(
                "Transaction {tx_hash} failed, but block number not found"
            ));
        }

        Ok(())
    }

    pub async fn construct_anchor_tx(
        &self,
        l2_slot_info: &L2SlotInfo,
        anchor_block_id: u64,
        anchor_state_root: B256,
        base_fee_config: BaseFeeConfig,
    ) -> Result<Transaction, Error> {
        self.construct_anchor_tx_impl(
            *l2_slot_info.parent_hash(),
            anchor_block_id,
            anchor_state_root,
            l2_slot_info.parent_gas_used(),
            base_fee_config,
            l2_slot_info.base_fee(),
        )
        .await
    }

    pub async fn construct_anchor_tx_impl(
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
        common::crypto::fixed_k_signer::sign_hash_deterministic(GOLDEN_TOUCH_PRIVATE_KEY, hash)
    }
}
