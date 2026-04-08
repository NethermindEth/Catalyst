use crate::l2::bindings::{Anchor, ICheckpointStore::Checkpoint};
use crate::shared_abi::bindings::{
    Bridge::{self, MessageSent},
    IBridge::Message,
    SignalSent,
};
use alloy::{
    consensus::{
        BlockHeader, SignableTransaction, Transaction as AnchorTransaction, TxEnvelope,
        transaction::Recovered,
    },
    eips::BlockNumberOrTag,
    primitives::{Address, B256, Bytes, FixedBytes},
    providers::{DynProvider, Provider},
    rpc::types::Transaction,
    signers::{Signature, Signer as AlloySigner},
    sol_types::SolEvent,
};
use anyhow::Error;
use common::shared::{
    alloy_tools, execution_layer::ExecutionLayer as ExecutionLayerCommon,
    l2_slot_info_v2::L2SlotInfoV2,
};
use common::{
    crypto::{GOLDEN_TOUCH_ADDRESS, GOLDEN_TOUCH_PRIVATE_KEY},
    signer::Signer,
};
use pacaya::l2::config::TaikoConfig;
use serde_json::Value;
use std::sync::Arc;
use tracing::{debug, info};

pub struct L2ExecutionLayer {
    common: ExecutionLayerCommon,
    pub provider: DynProvider,
    shasta_anchor: Anchor::AnchorInstance<DynProvider>,
    pub bridge: Bridge::BridgeInstance<DynProvider>,
    pub signal_service: Address,
    pub chain_id: u64,
    pub config: TaikoConfig,
    l2_call_signer: Arc<Signer>,
}

impl L2ExecutionLayer {
    pub async fn new(taiko_config: TaikoConfig) -> Result<Self, Error> {
        let provider =
            alloy_tools::create_alloy_provider_without_wallet(&taiko_config.taiko_geth_url).await?;

        let shasta_anchor = Anchor::new(taiko_config.taiko_anchor_address, provider.clone());

        let common =
            ExecutionLayerCommon::new(provider.clone(), taiko_config.signer.get_address()).await?;

        let chain_id = common.chain_id();
        info!("L2 chain ID {}", chain_id);

        // Surge: Store the bridge for processing L2 calls
        let chain_id_string = format!("{}", chain_id);
        let zeros_needed = 38usize.saturating_sub(chain_id_string.len());
        let bridge_address: Address =
            format!("0x{}{}01", chain_id_string, "0".repeat(zeros_needed)).parse()?;
        let bridge = Bridge::new(bridge_address, provider.clone());

        // Signal service address (same format as bridge, but ending in 05)
        let signal_service: Address =
            format!("0x{}{}05", chain_id_string, "0".repeat(zeros_needed)).parse()?;

        let l2_call_signer = taiko_config.signer.clone();

        Ok(Self {
            common,
            provider,
            shasta_anchor,
            bridge,
            signal_service,
            chain_id,
            l2_call_signer,
            config: taiko_config,
        })
    }

    pub fn common(&self) -> &ExecutionLayerCommon {
        &self.common
    }

    pub async fn construct_anchor_tx(
        &self,
        l2_slot_info: &L2SlotInfoV2,
        anchor_block_params: (Checkpoint, Vec<FixedBytes<32>>),
    ) -> Result<Transaction, Error> {
        let nonce = self
            .provider
            .get_transaction_count(GOLDEN_TOUCH_ADDRESS)
            .block_id((*l2_slot_info.parent_hash()).into())
            .await
            .map_err(|e| {
                self.common
                    .chain_error("Failed to get transaction count", Some(&e.to_string()))
            })?;

        let call_builder = self
            .shasta_anchor
            .anchorV4WithSignalSlots(anchor_block_params.0, anchor_block_params.1)
            .gas(1_000_000) // value expected by Taiko
            .max_fee_per_gas(u128::from(l2_slot_info.base_fee())) // value expected by Taiko
            .max_priority_fee_per_gas(0) // value expected by Taiko
            .nonce(nonce)
            .chain_id(self.common.chain_id());

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

        debug!(
            "AnchorTX transaction hash: {}, block number: {}",
            tx_envelope.tx_hash(),
            l2_slot_info.parent_id() + 1
        );

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

    pub async fn transfer_eth_from_l2_to_l1(
        &self,
        amount: u128,
        dest_chain_id: u64,
        preconfer_address: Address,
        bridge_relayer_fee: u64,
    ) -> Result<(), Error> {
        info!(
            "Transfer ETH from L2 to L1: srcChainId: {}, dstChainId: {}",
            self.common.chain_id(),
            dest_chain_id
        );

        let provider =
            alloy_tools::construct_alloy_provider(&self.config.signer, &self.config.taiko_geth_url)
                .await?;

        pacaya::l2::execution_layer::L2ExecutionLayer::transfer_eth_from_l2_to_l1_with_provider(
            self.config.taiko_bridge_address,
            provider,
            amount,
            self.common.chain_id(),
            dest_chain_id,
            preconfer_address,
            bridge_relayer_fee,
        )
        .await
        .map_err(|e| anyhow::anyhow!("Failed to transfer ETH from L2 to L1: {}", e))
    }

    pub async fn get_last_synced_proposal_id_from_geth(&self) -> Result<u64, Error> {
        self.get_proposal_id_from_geth(BlockNumberOrTag::Latest)
            .await
    }

    pub async fn get_proposal_id_from_geth_by_block_id(&self, block_id: u64) -> Result<u64, Error> {
        self.get_proposal_id_from_geth(BlockNumberOrTag::Number(block_id))
            .await
    }

    pub async fn get_latest_block_id_and_proposal_id(&self) -> Result<(u64, u64), Error> {
        let block = self
            .common
            .get_block_header(BlockNumberOrTag::Latest)
            .await?;
        let block_id = block.header.number;
        let proposal_id =
            super::extra_data::ExtraData::decode(block.header.extra_data())?.proposal_id;
        Ok((block_id, proposal_id))
    }

    pub async fn get_proposal_id_from_geth(&self, block: BlockNumberOrTag) -> Result<u64, Error> {
        let block = self.common.get_block_header(block).await?;
        let proposal_id =
            super::extra_data::ExtraData::decode(block.header.extra_data())?.proposal_id;
        Ok(proposal_id)
    }

    async fn get_anchor_transaction_input(
        &self,
        block: BlockNumberOrTag,
    ) -> Result<Vec<u8>, Error> {
        let block = self.common.get_block_with_txs(block).await?;
        let anchor_tx = match block.transactions.as_transactions() {
            Some(txs) => txs.first().ok_or_else(|| {
                anyhow::anyhow!(
                    "get_anchor_transaction_input: Cannot get anchor transaction from block {}",
                    block.number()
                )
            })?,
            None => {
                return Err(anyhow::anyhow!(
                    "No transactions in L2 block {}",
                    block.number()
                ));
            }
        };

        Ok(anchor_tx.input().to_vec())
    }

    pub async fn get_anchor_block_id_from_geth(&self, block_id: u64) -> Result<u64, Error> {
        // Genesis block (0) has no anchor transaction; return 0 as last anchor id.
        if block_id == 0 {
            return Ok(0);
        }
        self.get_anchor_transaction_input(BlockNumberOrTag::Number(block_id))
            .await
            .map_err(|e| anyhow::anyhow!("get_anchor_block_id_from_geth: {e}"))
            .and_then(|input| Self::decode_anchor_id_from_tx_data(&input))
    }

    pub fn decode_anchor_id_from_tx_data(data: &[u8]) -> Result<u64, Error> {
        match <Anchor::anchorV4WithSignalSlotsCall as alloy::sol_types::SolCall>::abi_decode_validate(data) {
            Ok(tx_data) => Ok(tx_data._checkpoint.blockNumber.to::<u64>()),
            Err(v4_error) => {
                let tx_data = <pacaya::l2::bindings::TaikoAnchor::anchorV3Call as alloy::sol_types::SolCall>::abi_decode_validate(data)
                    .map_err(|v3_error| anyhow::anyhow!(
                        "Failed to decode anchor id from tx data as anchorV4WithSignalSlots ({}) or anchorV3 ({}).",
                        v4_error,
                        v3_error
                    ))?;
                Ok(tx_data._anchorBlockId)
            }
        }
    }

    pub fn get_anchor_tx_data(data: &[u8]) -> Result<Anchor::anchorV4WithSignalSlotsCall, Error> {
        let tx_data =
            <Anchor::anchorV4WithSignalSlotsCall as alloy::sol_types::SolCall>::abi_decode_validate(data)
                .map_err(|e| anyhow::anyhow!("Failed to decode anchor tx data: {}", e))?;
        Ok(tx_data)
    }

    pub async fn get_head_l1_origin(&self) -> Result<u64, Error> {
        let response = self
            .provider
            .raw_request::<_, Value>(std::borrow::Cow::Borrowed("taiko_headL1Origin"), ())
            .await
            .map_err(|e| anyhow::anyhow!("Failed to fetch taiko_headL1Origin: {}", e))?;

        let hex_str = response
            .get("blockID")
            .or_else(|| response.get("blockId"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                anyhow::anyhow!("Missing or invalid  block id in taiko_headL1Origin response, allowed keys are: blockID, blockId")
            })?;

        u64::from_str_radix(hex_str.trim_start_matches("0x"), 16)
            .map_err(|e| anyhow::anyhow!("Failed to parse 'blockID' as u64: {}", e))
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

    pub async fn get_block_params_from_geth(&self, block_id: u64) -> Result<Checkpoint, Error> {
        self.get_anchor_transaction_input(BlockNumberOrTag::Number(block_id))
            .await
            .map_err(|e| anyhow::anyhow!("get_block_params_from_geth: {e}"))
            .and_then(|input| Self::decode_block_params_from_tx_data(&input))
    }

    pub fn decode_block_params_from_tx_data(data: &[u8]) -> Result<Checkpoint, Error> {
        let tx_data =
            <Anchor::anchorV4WithSignalSlotsCall as alloy::sol_types::SolCall>::abi_decode_validate(data)
                .map_err(|e| anyhow::anyhow!("Failed to decode proposal id from tx data: {}", e))?;
        Ok(tx_data._checkpoint)
    }
}

// Surge: L2 EL ops for Bridge Handler

#[allow(async_fn_in_trait)]
pub trait L2BridgeHandlerOps {
    // Surge: Builds the L2 call expected to be initiated an L1 contract via the Bridge
    // This is initially sent as a user op to the bridge handler RPC
    async fn construct_l2_call_tx(&self, message: Message) -> Result<Transaction, Error>;

    // Surge: This can be made to retrieve multiple signal slots
    async fn find_message_and_signal_slot(
        &self,
        block_id: u64,
    ) -> Result<Option<(Message, FixedBytes<32>)>, anyhow::Error>;
}

impl L2BridgeHandlerOps for L2ExecutionLayer {
    async fn construct_l2_call_tx(&self, message: Message) -> Result<Transaction, Error> {
        use alloy::signers::local::PrivateKeySigner;
        use std::str::FromStr;

        debug!("Constructing bridge call transaction for L2 call");

        let signer_address = self.l2_call_signer.get_address();

        let nonce = self
            .provider
            .get_transaction_count(signer_address)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get nonce for bridge call: {}", e))?;

        let call_builder = self
            .bridge
            .processMessage(message, Bytes::new())
            .gas(1_000_000)
            .max_fee_per_gas(1_000_000_000) // 1 gwei
            .max_priority_fee_per_gas(0)
            .nonce(nonce)
            .chain_id(self.chain_id);

        let typed_tx = call_builder
            .into_transaction_request()
            .build_typed_tx()
            .map_err(|_| anyhow::anyhow!("L2 Call Tx: Failed to build typed transaction"))?;

        let tx_eip1559 = typed_tx
            .eip1559()
            .ok_or_else(|| anyhow::anyhow!("L2 Call Tx: Failed to extract EIP-1559 transaction"))?
            .clone();

        // Sign the transaction using the L2 call signer
        let signature = match self.l2_call_signer.as_ref() {
            Signer::Web3signer(web3signer, address) => {
                let signature_bytes = web3signer.sign_transaction(&tx_eip1559, *address).await?;
                Signature::try_from(signature_bytes.as_slice())
                    .map_err(|e| anyhow::anyhow!("Failed to parse signature: {}", e))?
            }
            Signer::PrivateKey(private_key, _) => {
                let signer = PrivateKeySigner::from_str(private_key.as_str())?;
                AlloySigner::sign_hash(&signer, &tx_eip1559.signature_hash()).await?
            }
        };

        let sig_tx = tx_eip1559.into_signed(signature);

        let tx_envelope = TxEnvelope::from(sig_tx);

        debug!("L2 Call transaction hash: {}", tx_envelope.tx_hash());

        let tx = Transaction {
            inner: Recovered::new_unchecked(tx_envelope, signer_address),
            block_hash: None,
            block_number: None,
            transaction_index: None,
            effective_gas_price: None,
        };
        Ok(tx)
    }

    async fn find_message_and_signal_slot(
        &self,
        block_id: u64,
    ) -> Result<Option<(Message, FixedBytes<32>)>, anyhow::Error> {
        use alloy::rpc::types::Filter;

        let bridge_address = *self.bridge.address();
        let signal_service_address = self.signal_service;

        let filter = Filter::new().from_block(block_id).to_block(block_id);

        // Get logs from the bridge contract (MessageSent event)
        let bridge_filter = filter
            .clone()
            .address(bridge_address)
            .event_signature(MessageSent::SIGNATURE_HASH);

        let bridge_logs = self
            .provider
            .get_logs(&bridge_filter)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get MessageSent logs from bridge: {e}"))?;

        // Get logs from the signal service contract (SignalSent event)
        let signal_filter = filter
            .address(signal_service_address)
            .event_signature(SignalSent::SIGNATURE_HASH);

        let signal_logs = self.provider.get_logs(&signal_filter).await.map_err(|e| {
            anyhow::anyhow!("Failed to get SignalSent logs from signal service: {e}")
        })?;

        // Check if both events are present
        if bridge_logs.is_empty() || signal_logs.is_empty() {
            return Ok(None);
        }

        // Decode MessageSent event
        let message = {
            let log = bridge_logs
                .first()
                .ok_or_else(|| anyhow::anyhow!("bridge_logs is empty despite non-empty check"))?;
            let log_data = alloy::primitives::LogData::new_unchecked(
                log.topics().to_vec(),
                log.data().data.clone(),
            );
            MessageSent::decode_log_data(&log_data)
                .map_err(|e| anyhow::anyhow!("Failed to decode MessageSent event: {e}"))?
                .message
        };

        // Decode SignalSent event
        let slot = {
            let log = signal_logs
                .first()
                .ok_or_else(|| anyhow::anyhow!("signal_logs is empty despite non-empty check"))?;
            let log_data = alloy::primitives::LogData::new_unchecked(
                log.topics().to_vec(),
                log.data().data.clone(),
            );
            SignalSent::decode_log_data(&log_data)
                .map_err(|e| anyhow::anyhow!("Failed to decode SignalSent event: {e}"))?
                .slot
        };

        Ok(Some((message, slot)))
    }
}
