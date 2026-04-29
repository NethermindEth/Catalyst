use crate::l2::bindings::{Anchor, ICheckpointStore::Checkpoint};
use crate::shared_abi::bindings::{
    Bridge::{self, MessageSent},
    HopProof,
    IBridge::Message,
    SignalService::SignalSent,
};
use alloy::{
    consensus::{
        SignableTransaction, Transaction as AnchorTransaction, TxEnvelope, transaction::Recovered,
    },
    eips::{BlockId, BlockNumberOrTag},
    primitives::{Address, B256, Bytes, FixedBytes},
    providers::{DynProvider, Provider, ext::DebugApi},
    rpc::types::{
        Transaction, TransactionRequest,
        trace::geth::{
            CallFrame, GethDebugBuiltInTracerType, GethDebugTracerType,
            GethDebugTracingCallOptions, GethDebugTracingOptions,
        },
    },
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
use std::sync::Arc;
use tracing::{debug, info, warn};

pub struct L2ExecutionLayer {
    common: ExecutionLayerCommon,
    pub provider: DynProvider,
    anchor: Anchor::AnchorInstance<DynProvider>,
    pub bridge: Bridge::BridgeInstance<DynProvider>,
    pub signal_service: Address,
    pub chain_id: u64,
    l2_call_signer: Arc<Signer>,
}

impl L2ExecutionLayer {
    pub async fn new(
        taiko_config: TaikoConfig,
        bridge_address: Address,
        signal_service: Address,
    ) -> Result<Self, Error> {
        let provider =
            alloy_tools::create_alloy_provider_without_wallet(&taiko_config.taiko_geth_url).await?;

        let chain_id = provider
            .get_chain_id()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get chain ID: {}", e))?;
        info!("L2 Chain ID: {}", chain_id);

        let anchor = Anchor::new(taiko_config.taiko_anchor_address, provider.clone());
        let bridge = Bridge::new(bridge_address, provider.clone());

        let common =
            ExecutionLayerCommon::new(provider.clone(), taiko_config.signer.get_address()).await?;
        let l2_call_signer = taiko_config.signer.clone();

        Ok(Self {
            common,
            provider,
            anchor,
            bridge,
            signal_service,
            chain_id,
            l2_call_signer,
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
        debug!(
            "Constructing anchor transaction for block number: {}",
            l2_slot_info.parent_id() + 1
        );
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
            .anchor
            .anchorV4WithSignalSlots(anchor_block_params.0, anchor_block_params.1)
            .gas(1_000_000)
            .max_fee_per_gas(u128::from(l2_slot_info.base_fee()))
            .max_priority_fee_per_gas(0)
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

    pub async fn transfer_eth_from_l2_to_l1(
        &self,
        _amount: u128,
        _dest_chain_id: u64,
        _preconfer_address: Address,
        _bridge_relayer_fee: u64,
    ) -> Result<(), Error> {
        warn!("Implement bridge transfer logic here");
        Ok(())
    }

    pub async fn get_last_synced_anchor_block_id_from_geth(&self) -> Result<u64, Error> {
        self.get_latest_anchor_transaction_input()
            .await
            .map_err(|e| anyhow::anyhow!("get_last_synced_anchor_block_id_from_geth: {e}"))
            .and_then(|input| Self::decode_anchor_id_from_tx_data(&input))
    }

    async fn get_latest_anchor_transaction_input(&self) -> Result<Vec<u8>, Error> {
        let block = self.common.get_latest_block_with_txs().await?;
        let anchor_tx = match block.transactions.as_transactions() {
            Some(txs) => txs.first().ok_or_else(|| {
                anyhow::anyhow!(
                    "get_latest_anchor_transaction_input: Cannot get anchor transaction from block {}",
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

    pub fn decode_anchor_id_from_tx_data(data: &[u8]) -> Result<u64, Error> {
        let tx_data =
            <Anchor::anchorV4WithSignalSlotsCall as alloy::sol_types::SolCall>::abi_decode_validate(
                data,
            )
            .map_err(|e| anyhow::anyhow!("Failed to decode anchor id from tx data: {}", e))?;
        Ok(tx_data._checkpoint.blockNumber.to::<u64>())
    }
}

// Surge: L2 EL ops for Bridge Handler

pub trait L2BridgeHandlerOps {
    async fn construct_l2_call_tx(&self, message: Message) -> Result<Transaction, Error>;
    async fn find_message_and_signal_slot(
        &self,
        block_id: u64,
    ) -> Result<Option<(Message, FixedBytes<32>)>, anyhow::Error>;
    async fn get_hop_proof(
        &self,
        slot: FixedBytes<32>,
        block_id: u64,
        state_root: B256,
    ) -> Result<Bytes, anyhow::Error>;
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
            .gas(3_000_000)
            .max_fee_per_gas(1_000_000_000)
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

        let bridge_filter = filter
            .clone()
            .address(bridge_address)
            .event_signature(MessageSent::SIGNATURE_HASH);

        let bridge_logs = self
            .provider
            .get_logs(&bridge_filter)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get MessageSent logs from bridge: {e}"))?;

        let signal_filter = filter
            .address(signal_service_address)
            .event_signature(SignalSent::SIGNATURE_HASH);

        let signal_logs = self.provider.get_logs(&signal_filter).await.map_err(|e| {
            anyhow::anyhow!("Failed to get SignalSent logs from signal service: {e}")
        })?;

        if bridge_logs.is_empty() || signal_logs.is_empty() {
            return Ok(None);
        }

        let message = {
            let log = bridge_logs
                .first()
                .ok_or_else(|| anyhow::anyhow!("No bridge logs"))?;
            let log_data = alloy::primitives::LogData::new_unchecked(
                log.topics().to_vec(),
                log.data().data.clone(),
            );
            MessageSent::decode_log_data(&log_data)
                .map_err(|e| anyhow::anyhow!("Failed to decode MessageSent event: {e}"))?
                .message
        };

        let slot = {
            let log = signal_logs
                .first()
                .ok_or_else(|| anyhow::anyhow!("No signal logs"))?;
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

    async fn get_hop_proof(
        &self,
        slot: FixedBytes<32>,
        block_id: u64,
        state_root: B256,
    ) -> Result<Bytes, anyhow::Error> {
        use alloy::sol_types::SolValue;

        let proof = self
            .provider
            .get_proof(self.signal_service, vec![slot])
            .block_id(block_id.into())
            .await
            .map_err(|e| anyhow::anyhow!("eth_getProof failed for signal slot: {e}"))?;

        let storage_proof = proof
            .storage_proof
            .first()
            .ok_or_else(|| anyhow::anyhow!("No storage proof returned for signal slot"))?;

        let hop_proof = HopProof {
            chainId: self.chain_id,
            blockId: block_id,
            rootHash: state_root,
            cacheOption: 0,
            accountProof: proof.account_proof.clone(),
            storageProof: storage_proof.proof.clone(),
        };

        info!(
            "Built HopProof: chainId={}, blockId={}, rootHash={}, accountProof_len={}, storageProof_len={}",
            hop_proof.chainId,
            hop_proof.blockId,
            hop_proof.rootHash,
            hop_proof.accountProof.len(),
            hop_proof.storageProof.len(),
        );

        Ok(Bytes::from(vec![hop_proof].abi_encode_params()))
    }
}

// Surge: L2 mempool tx scanning and simulation

/// `Bridge.sendMessage(Message)` selector — used for call-based detection
/// in the trace tree because the L2 bridge is behind a DELEGATECALL proxy
/// and the Nethermind callTracer doesn't surface event logs from proxied calls.
const SEND_MESSAGE_SELECTOR: [u8; 4] = [0x1b, 0xdb, 0x00, 0x37];

impl L2ExecutionLayer {
    /// Trace a transaction to detect any `Bridge.sendMessage` call it makes.
    /// Instead of relying on `MessageSent` event logs (which the L2 Nethermind
    /// callTracer doesn't emit through DELEGATECALL proxies), we scan the call
    /// tree for CALL frames targeting the L2 bridge with the `sendMessage`
    /// selector, and decode the Message from the call input.
    pub async fn trace_tx_for_outbound_message(
        &self,
        from: Address,
        to: Address,
        input: &[u8],
        value: Option<alloy::primitives::U256>,
    ) -> Result<Option<Message>, anyhow::Error> {
        let mut tx_request = TransactionRequest::default()
            .from(from)
            .to(to)
            .input(input.to_vec().into());

        if let Some(v) = value {
            tx_request = tx_request.value(v);
        }

        let tracer_config = serde_json::json!({
            "onlyTopCall": false
        });

        let tracing_options = GethDebugTracingOptions {
            tracer: Some(GethDebugTracerType::BuiltInTracer(
                GethDebugBuiltInTracerType::CallTracer,
            )),
            tracer_config: tracer_config.into(),
            ..Default::default()
        };

        let call_options = GethDebugTracingCallOptions {
            tracing_options,
            ..Default::default()
        };

        let trace_result = match self
            .provider
            .debug_trace_call(
                tx_request,
                BlockId::Number(BlockNumberOrTag::Latest),
                call_options,
            )
            .await
        {
            Ok(t) => t,
            Err(e) => {
                return Err(anyhow::anyhow!("L2 tx trace RPC failed: {e}"));
            }
        };

        let bridge_address = *self.bridge.address();
        let mut message: Option<Message> = None;
        let mut send_message_caller: Option<Address> = None;

        if let alloy::rpc::types::trace::geth::GethTrace::CallTracer(call_frame) = trace_result {
            // Walk the call tree looking for CALL frames to the bridge with
            // the sendMessage selector. The Message struct is ABI-encoded as
            // the first (and only) parameter after the 4-byte selector.
            if let Some((msg, caller)) = find_send_message_in_calls(&call_frame, bridge_address) {
                message = Some(msg);
                send_message_caller = Some(caller);
            }
        }

        if let Some(ref mut m) = message {
            // The bridge fills `from`, `srcChainId`, and `id` during sendMessage
            // execution, but the call-based detection reads the INPUT before
            // those are set. Patch them with what the bridge would assign.
            m.from = send_message_caller.unwrap_or(from);
            m.srcChainId = self.chain_id;
            // For `id`, query the bridge's nextMessageId (this is what it would assign)
            if let Ok(next_id) = self.bridge.nextMessageId().call().await {
                m.id = next_id;
            }

            debug!(
                "L2 trace found outbound sendMessage: destChainId={}, to={}, from={}",
                m.destChainId, m.to, m.from
            );
        } else {
            debug!("L2 trace found no outbound sendMessage");
        }

        Ok(message)
    }
}

/// Recursively search call frames for a CALL to `bridge_address` with the
/// `sendMessage` function selector. Returns the decoded Message and the
/// caller address (msg.sender of the sendMessage call).
fn find_send_message_in_calls(
    frame: &CallFrame,
    bridge_address: Address,
) -> Option<(Message, Address)> {
    use crate::shared_abi::bindings::Bridge;
    use alloy::sol_types::SolCall;

    // Check this frame: is it a CALL to the bridge with sendMessage selector?
    if let Some(to_addr) = frame.to
        && to_addr == bridge_address
    {
        let input = frame.input.as_ref();
        if input.len() >= 4
            && input[0..4] == SEND_MESSAGE_SELECTOR
            && let Ok(decoded) = Bridge::sendMessageCall::abi_decode_raw(&input[4..])
        {
            // `frame.from` is the msg.sender of this call
            let caller = frame.from;
            return Some((decoded._message, caller));
        }
    }

    for sub in &frame.calls {
        if let Some(result) = find_send_message_in_calls(sub, bridge_address) {
            return Some(result);
        }
    }

    None
}
