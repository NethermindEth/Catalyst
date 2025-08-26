use super::{
    bindings::{Bridge, LibSharedData, TaikoAnchor},
    config::{GOLDEN_TOUCH_ADDRESS, GOLDEN_TOUCH_PRIVATE_KEY, TaikoConfig},
    fixed_k_signer_chainbound,
};
use crate::shared::alloy_tools;
use alloy::{
    consensus::{
        SignableTransaction, Transaction as AnchorTransaction, TxEnvelope, transaction::Recovered,
    },
    contract::Error as ContractError,
    eips::BlockNumberOrTag,
    network::ReceiptResponse,
    primitives::{Address, B256, Bytes, U256, Uint},
    providers::{DynProvider, Provider},
    rpc::types::{Block as RpcBlock, Transaction},
    signers::Signature,
    transports::TransportErrorKind,
};
use alloy_json_rpc::RpcError;
use anyhow::Error;
use serde_json::Value;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

pub struct L2ExecutionLayer {
    provider: RwLock<DynProvider>,
    taiko_anchor: RwLock<TaikoAnchor::TaikoAnchorInstance<DynProvider>>,
    chain_id: u64,
    config: TaikoConfig,
}

impl L2ExecutionLayer {
    pub async fn new(taiko_config: TaikoConfig) -> Result<Self, Error> {
        let provider = RwLock::new(
            alloy_tools::create_alloy_provider_without_wallet(&taiko_config.taiko_geth_url).await?,
        );

        let chain_id = provider
            .read()
            .await
            .get_chain_id()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get chain ID: {}", e))?;
        info!("L2 Chain ID: {}", chain_id);

        let taiko_anchor = RwLock::new(TaikoAnchor::new(
            taiko_config.taiko_anchor_address,
            provider.read().await.clone(),
        ));

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
        let block_by_number = self.provider.read().await.get_block_by_number(block).await;

        self.check_for_provider_failure(block_by_number, "Failed to get latest L2 block")
            .await?
            .ok_or(anyhow::anyhow!("Failed to get latest L2 block"))
    }

    async fn get_latest_l2_block_with_txs(&self) -> Result<RpcBlock, Error> {
        let block_by_number = self
            .provider
            .read()
            .await
            .get_block_by_number(BlockNumberOrTag::Latest)
            .full()
            .await;

        self.check_for_provider_failure(block_by_number, "Failed to get latest L2 block")
            .await?
            .ok_or(anyhow::anyhow!("Failed to get latest L2 block"))
    }

    pub async fn get_balance(&self, address: Address) -> Result<U256, Error> {
        let balance = self.provider.read().await.get_balance(address).await;
        self.check_for_provider_failure(balance, "Failed to get L2 balance")
            .await
    }

    pub async fn get_forced_inclusion_form_l1origin(&self, block_id: u64) -> Result<bool, Error> {
        let result = self
            .provider
            .read()
            .await
            .raw_request(
                std::borrow::Cow::Borrowed("taiko_l1OriginByID"),
                vec![Value::String(block_id.to_string())],
            )
            .await;

        let is_forced_inclusion: bool = self
            .check_for_provider_failure::<Value>(result, "Failed to get forced inclusion")
            .await?
            .get("isForcedInclusion")
            .and_then(Value::as_bool)
            .ok_or_else(|| anyhow::anyhow!("Failed to parse isForcedInclusion"))?;

        Ok(is_forced_inclusion)
    }

    pub async fn get_latest_l2_block_id(&self) -> Result<u64, Error> {
        let block_number = self.provider.read().await.get_block_number().await;

        self.check_for_provider_failure(block_number, "Failed to get latest L2 block number")
            .await
    }

    pub async fn get_l2_block_by_number(
        &self,
        number: u64,
        full_txs: bool,
    ) -> Result<alloy::rpc::types::Block, Error> {
        let mut block_by_number = self
            .provider
            .read()
            .await
            .get_block_by_number(BlockNumberOrTag::Number(number));

        if full_txs {
            block_by_number = block_by_number.full();
        }

        let block = self
            .check_for_provider_failure(block_by_number.await, "Failed to get L2 block by number")
            .await?
            .ok_or_else(|| anyhow::anyhow!("Failed to get L2 block {}: value was None", number))?;
        Ok(block)
    }

    pub async fn construct_anchor_tx(
        &self,
        parent_hash: B256,
        anchor_block_id: u64,
        anchor_state_root: B256,
        parent_gas_used: u32,
        base_fee_config: LibSharedData::BaseFeeConfig,
        base_fee: u64,
    ) -> Result<Transaction, Error> {
        // Create the contract call
        let taiko_anchor = self.taiko_anchor.read().await;
        let tx_count_result = self
            .provider
            .read()
            .await
            .get_transaction_count(GOLDEN_TOUCH_ADDRESS)
            .block_id(parent_hash.into())
            .await;
        let nonce = self
            .check_for_provider_failure(tx_count_result, "Failed to get nonce")
            .await?;
        let call_builder = taiko_anchor
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
        fixed_k_signer_chainbound::sign_hash_deterministic(GOLDEN_TOUCH_PRIVATE_KEY, hash)
    }

    pub async fn get_transaction_by_hash(
        &self,
        hash: B256,
    ) -> Result<alloy::rpc::types::Transaction, Error> {
        let transaction_by_hash = self
            .provider
            .read()
            .await
            .get_transaction_by_hash(hash)
            .await;

        let transaction = self
            .check_for_provider_failure(transaction_by_hash, "Failed to get L2 transaction by hash")
            .await?
            .ok_or(anyhow::anyhow!(
                "Failed to get L2 transaction: value is None"
            ))?;
        Ok(transaction)
    }

    pub async fn get_base_fee(
        &self,
        parent_hash: B256,
        parent_gas_used: u32,
        base_fee_config: LibSharedData::BaseFeeConfig,
        l2_slot_timestamp: u64,
    ) -> Result<u64, Error> {
        let base_fee_v2_result = self
            .taiko_anchor
            .read()
            .await
            .getBasefeeV2(parent_gas_used, l2_slot_timestamp, base_fee_config)
            .block(parent_hash.into())
            .call()
            .await;
        let base_fee = self
            .check_for_contract_failure(base_fee_v2_result, "Failed to get base fee")
            .await?
            .basefee_;

        base_fee
            .try_into()
            .map_err(|err| anyhow::anyhow!("Failed to convert base fee to u64: {}", err))
    }

    pub async fn get_last_synced_anchor_block_id_from_taiko_anchor(&self) -> Result<u64, Error> {
        let last_synced_block = self
            .taiko_anchor
            .read()
            .await
            .lastSyncedBlock()
            .call()
            .await;
        self.check_for_contract_failure(last_synced_block, "Failed to get last synced block")
            .await
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

    /// Warning: be sure not to `read` from the rwlock
    /// while passing parameters to this function
    async fn check_for_provider_failure<T>(
        &self,
        result: Result<T, RpcError<TransportErrorKind>>,
        error_message: &str,
    ) -> Result<T, Error> {
        match result {
            Ok(result) => Ok(result),
            Err(e) => {
                self.recreate_provider().await?;
                Err(anyhow::anyhow!(
                    "{}. Recreating WebSocket provider. Transport error: {}",
                    error_message,
                    e
                ))
            }
        }
    }

    /// Warning: be sure not to `read` from the rwlock
    /// while passing parameters to this function
    async fn check_for_contract_failure<T>(
        &self,
        result: Result<T, ContractError>,
        error_message: &str,
    ) -> Result<T, Error> {
        match result {
            Ok(result) => Ok(result),
            Err(ContractError::TransportError(e)) => {
                self.recreate_provider().await?;
                Err(anyhow::anyhow!(
                    "{}. Recreating WebSocket provider. Transport error: {}",
                    error_message,
                    e
                ))
            }
            Err(e) => Err(anyhow::anyhow!("{}: {}", error_message, e)),
        }
    }

    async fn recreate_provider(&self) -> Result<(), Error> {
        let provider =
            alloy_tools::create_alloy_provider_without_wallet(&self.config.taiko_geth_url).await?;

        *self.taiko_anchor.write().await =
            TaikoAnchor::new(self.config.taiko_anchor_address, provider.clone());
        *self.provider.write().await = provider;
        debug!(
            "Created new WebSocket provider for {}",
            self.config.taiko_geth_url
        );
        Ok(())
    }
}
