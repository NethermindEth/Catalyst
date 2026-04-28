use super::config::EthereumL1Config;
use super::proposal_tx_builder::ProposalTxBuilder;
use super::protocol_config::ProtocolConfig;
use crate::l1::bindings::RealTimeInbox::{self, RealTimeInboxInstance};
use crate::node::proposal_manager::proposal::Proposal;
use crate::raiko::RaikoClient;
use crate::shared_abi::bindings::{
    Bridge, Bridge::MessageSent, IBridge::Message, SignalService::SignalSent,
};
use crate::{l1::config::ContractAddresses, node::proposal_manager::bridge_handler::UserOp};
use alloy::{
    eips::{BlockId, BlockNumberOrTag},
    primitives::{Address, B256, Bytes, FixedBytes},
    providers::{DynProvider, ext::DebugApi},
    rpc::types::{
        TransactionRequest,
        trace::geth::{
            GethDebugBuiltInTracerType, GethDebugTracerType, GethDebugTracingCallOptions,
            GethDebugTracingOptions,
        },
    },
    sol_types::SolEvent,
};
use anyhow::{Error, anyhow};
use common::{
    l1::{
        traits::{ELTrait, PreconferProvider},
        transaction_error::TransactionError,
    },
    metrics::Metrics,
    shared::{
        alloy_tools, execution_layer::ExecutionLayer as ExecutionLayerCommon,
        transaction_monitor::TransactionMonitor,
    },
};
use pacaya::l1::{operators_cache::OperatorError, traits::PreconfOperator};
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tracing::info;

pub struct ExecutionLayer {
    common: ExecutionLayerCommon,
    provider: DynProvider,
    preconfer_address: Address,
    pub transaction_monitor: TransactionMonitor,
    contract_addresses: ContractAddresses,
    realtime_inbox: RealTimeInboxInstance<DynProvider>,
    #[allow(dead_code)]
    raiko_client: RaikoClient,
    proof_type: crate::l1::bindings::ProofType,
    mock_mode: bool,
    extra_gas_percentage: u64,
}

impl ELTrait for ExecutionLayer {
    type Config = EthereumL1Config;
    async fn new(
        common_config: common::l1::config::EthereumL1Config,
        specific_config: Self::Config,
        transaction_error_channel: Sender<TransactionError>,
        metrics: Arc<Metrics>,
    ) -> Result<Self, Error> {
        let provider = alloy_tools::construct_alloy_provider(
            &common_config.signer,
            common_config
                .execution_rpc_urls
                .first()
                .ok_or_else(|| anyhow!("L1 RPC URL is required"))?,
        )
        .await?;
        let common =
            ExecutionLayerCommon::new(provider.clone(), common_config.signer.get_address()).await?;

        let transaction_monitor = TransactionMonitor::new(
            provider.clone(),
            &common_config,
            transaction_error_channel,
            metrics.clone(),
            common.chain_id(),
        )
        .await
        .map_err(|e| Error::msg(format!("Failed to create TransactionMonitor: {e}")))?;

        let realtime_inbox = RealTimeInbox::new(specific_config.realtime_inbox, provider.clone());

        let config = realtime_inbox
            .getConfig()
            .call()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to call getConfig for RealTimeInbox: {e}"))?;

        tracing::info!(
            "RealTimeInbox: {}, proofVerifier: {}, signalService: {}",
            specific_config.realtime_inbox,
            config.proofVerifier,
            config.signalService,
        );

        let contract_addresses = ContractAddresses {
            realtime_inbox: specific_config.realtime_inbox,
            proposer_multicall: specific_config.proposer_multicall,
            bridge: specific_config.bridge,
            signal_service: specific_config.signal_service,
        };

        let proof_type = specific_config.proof_type;
        let mock_mode = specific_config.mock_mode;
        let raiko_client = specific_config.raiko_client;
        let extra_gas_percentage = common_config.extra_gas_percentage;

        Ok(Self {
            common,
            provider,
            preconfer_address: common_config.signer.get_address(),
            transaction_monitor,
            contract_addresses,
            realtime_inbox,
            raiko_client,
            proof_type,
            mock_mode,
            extra_gas_percentage,
        })
    }

    fn common(&self) -> &ExecutionLayerCommon {
        &self.common
    }
}

impl PreconferProvider for ExecutionLayer {
    async fn get_preconfer_wallet_eth(&self) -> Result<alloy::primitives::U256, Error> {
        self.common()
            .get_account_balance(self.preconfer_address)
            .await
    }

    async fn get_preconfer_nonce_pending(&self) -> Result<u64, Error> {
        self.common()
            .get_account_nonce(self.preconfer_address, BlockNumberOrTag::Pending)
            .await
    }

    async fn get_preconfer_nonce_latest(&self) -> Result<u64, Error> {
        self.common()
            .get_account_nonce(self.preconfer_address, BlockNumberOrTag::Latest)
            .await
    }

    fn get_preconfer_address(&self) -> Address {
        self.preconfer_address
    }
}

impl PreconfOperator for ExecutionLayer {
    fn get_preconfer_address(&self) -> Address {
        self.preconfer_address
    }

    async fn get_operators_for_current_and_next_epoch(
        &self,
        _current_epoch_timestamp: u64,
        _current_slot_timestamp: u64,
    ) -> Result<(Address, Address), OperatorError> {
        // RealTime: anyone can propose, but we still use operator tracking for slot management.
        // Return self as both current and next operator.
        Ok((self.preconfer_address, self.preconfer_address))
    }

    async fn is_preconf_router_specified_in_taiko_wrapper(&self) -> Result<bool, Error> {
        Ok(true)
    }

    async fn get_l2_height_from_taiko_inbox(&self) -> Result<u64, Error> {
        Ok(0)
    }

    async fn get_handover_window_slots(&self) -> Result<u64, Error> {
        Err(anyhow::anyhow!(
            "Not implemented for RealTime execution layer"
        ))
    }
}

impl ExecutionLayer {
    #[allow(dead_code)]
    pub fn get_raiko_client(&self) -> &RaikoClient {
        &self.raiko_client
    }

    /// Returns a clone of the configured contract addresses (L1 inbox,
    /// bridge, signal service, proposer multicall). Useful for callers that
    /// need to reference these during block building.
    pub fn contract_addresses(&self) -> ContractAddresses {
        self.contract_addresses.clone()
    }

    pub async fn send_batch_to_l1(
        &self,
        batch: Proposal,
        tx_hash_notifier: Option<tokio::sync::oneshot::Sender<alloy::primitives::B256>>,
        tx_result_notifier: Option<tokio::sync::oneshot::Sender<bool>>,
    ) -> Result<(), Error> {
        info!(
            "📦 Proposing with {} blocks | user_ops: {:?} | signal_slots: {:?} | l1_calls: {:?} | zk_proof: {}",
            batch.l2_blocks.len(),
            batch.user_ops,
            batch.signal_slots,
            batch.l1_calls,
            batch.zk_proof.is_some(),
        );

        let builder = ProposalTxBuilder::new(
            self.provider.clone(),
            self.extra_gas_percentage,
            self.proof_type,
            self.mock_mode,
        );

        let tx = builder
            .build_propose_tx(
                batch,
                self.preconfer_address,
                self.contract_addresses.clone(),
            )
            .await?;

        let pending_nonce = self.get_preconfer_nonce_pending().await?;
        self.transaction_monitor
            .monitor_new_transaction(tx, pending_nonce, tx_hash_notifier, tx_result_notifier)
            .await
            .map_err(|e| Error::msg(format!("Sending batch to L1 failed: {e}")))?;

        Ok(())
    }

    pub async fn is_transaction_in_progress(&self) -> Result<bool, Error> {
        self.transaction_monitor.is_transaction_in_progress().await
    }

    pub async fn fetch_protocol_config(&self) -> Result<ProtocolConfig, Error> {
        let config = self
            .realtime_inbox
            .getConfig()
            .call()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to call getConfig for RealTimeInbox: {e}"))?;

        info!(
            "RealTimeInbox config: basefeeSharingPctg: {}",
            config.basefeeSharingPctg,
        );

        Ok(ProtocolConfig::from(&config))
    }

    pub async fn get_last_finalized_block_hash(&self) -> Result<B256, Error> {
        let result = self
            .realtime_inbox
            .getLastFinalizedBlockHash()
            .call()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to call getLastFinalizedBlockHash: {e}"))?;

        Ok(result)
    }
}

// Surge: L1 EL ops for Bridge Handler

use alloy::rpc::types::trace::geth::{CallFrame, CallLogFrame};

fn collect_all_logs(frame: &CallFrame) -> Vec<CallLogFrame> {
    let mut logs = Vec::new();
    let mut stack = vec![frame];

    while let Some(f) = stack.pop() {
        logs.extend(f.logs.iter().cloned());
        stack.extend(f.calls.iter());
    }

    logs
}

pub trait L1BridgeHandlerOps {
    async fn find_message_and_signal_slot(
        &self,
        user_op: UserOp,
    ) -> Result<Option<(Message, FixedBytes<32>)>, anyhow::Error>;

    /// Simulate `Bridge.processMessage(msg, proof)` on L1 and inspect the trace
    /// for any `MessageSent` event the invoked L1 callback emits. If it does,
    /// the return message is an L1→L2 bridge message that the originating L2
    /// block expects to consume as a fast signal — the slot of that return
    /// signal is what the inbox's `requiredReturnSignals` list must include.
    ///
    /// Returns `Some((return_message, return_signal_slot))` if a return is
    /// produced, `None` otherwise. Returns an error only for RPC failures; a
    /// callback that reverts during simulation yields `None` (no signal).
    async fn simulate_l1_callback_return_signal(
        &self,
        message_from_l2: Message,
        signal_slot_proof: Bytes,
        bridge_address: Address,
        l2_bridge_address: Address,
    ) -> Result<Option<(Message, FixedBytes<32>)>, anyhow::Error>;
}

impl L1BridgeHandlerOps for ExecutionLayer {
    async fn find_message_and_signal_slot(
        &self,
        user_op_data: UserOp,
    ) -> Result<Option<(Message, FixedBytes<32>)>, anyhow::Error> {
        let tx_request = TransactionRequest::default()
            .from(self.preconfer_address)
            .to(user_op_data.submitter)
            .input(user_op_data.calldata.into());

        let mut tracer_config = serde_json::Map::new();
        tracer_config.insert("withLog".to_string(), serde_json::Value::Bool(true));
        tracer_config.insert("onlyTopCall".to_string(), serde_json::Value::Bool(false));

        let tracing_options = GethDebugTracingOptions {
            tracer: Some(GethDebugTracerType::BuiltInTracer(
                GethDebugBuiltInTracerType::CallTracer,
            )),
            tracer_config: serde_json::Value::Object(tracer_config).into(),
            ..Default::default()
        };

        let call_options = GethDebugTracingCallOptions {
            tracing_options,
            ..Default::default()
        };

        let trace_result = self
            .provider
            .debug_trace_call(
                tx_request,
                BlockId::Number(BlockNumberOrTag::Latest),
                call_options,
            )
            .await
            .map_err(|e| anyhow!("Failed to simulate executeBatch on L1: {e}"))?;

        tracing::debug!("Received trace result: {:?}", trace_result);

        let mut message: Option<Message> = None;
        let mut slot: Option<FixedBytes<32>> = None;

        if let alloy::rpc::types::trace::geth::GethTrace::CallTracer(call_frame) = trace_result {
            let all_logs = collect_all_logs(&call_frame);
            tracing::debug!("Collected {} logs from call trace", all_logs.len());

            for log in all_logs {
                if let Some(topics) = &log.topics
                    && !topics.is_empty()
                {
                    if topics[0] == MessageSent::SIGNATURE_HASH {
                        let log_data = alloy::primitives::LogData::new_unchecked(
                            topics.clone(),
                            log.data.clone().unwrap_or_default(),
                        );
                        let decoded = MessageSent::decode_log_data(&log_data)
                            .map_err(|e| anyhow!("Failed to decode MessageSent event L1: {e}"))?;

                        message = Some(decoded.message);
                    } else if topics[0] == SignalSent::SIGNATURE_HASH {
                        let log_data = alloy::primitives::LogData::new_unchecked(
                            topics.clone(),
                            log.data.clone().unwrap_or_default(),
                        );
                        let decoded = SignalSent::decode_log_data(&log_data)
                            .map_err(|e| anyhow!("Failed to decode SignalSent event L1: {e}"))?;

                        slot = Some(decoded.slot);
                    }
                }
            }
        }

        tracing::debug!("{:?} {:?}", message, slot);

        if let (Some(message), Some(slot)) = (message, slot) {
            return Ok(Some((message, slot)));
        }

        Ok(None)
    }

    async fn simulate_l1_callback_return_signal(
        &self,
        message_from_l2: Message,
        _signal_slot_proof: Bytes,
        bridge_address: Address,
        _l2_bridge_address: Address,
    ) -> Result<Option<(Message, FixedBytes<32>)>, anyhow::Error> {
        use alloy::primitives::{B256, U256, keccak256};
        use alloy::rpc::types::state::{AccountOverride, StateOverride};

        // Instead of simulating Bridge.processMessage (which requires L1
        // signal verification we can't bypass), we call the L1 callback's
        // onMessageInvocation(data) directly with from=bridge. To make
        // bridge.context() return the correct values, we state-override the
        // bridge's __ctx storage (slots 253-254, see Bridge_Layout.sol):
        //   slot 253: msgHash (bytes32)
        //   slot 254: from (address, 20 bytes) | srcChainId (uint64, 8 bytes)

        let bridge = Bridge::new(bridge_address, self.provider.clone());
        let msg_hash: B256 = bridge
            .hashMessage(message_from_l2.clone())
            .call()
            .await
            .map_err(|e| anyhow!("Failed to call Bridge.hashMessage for sim: {e}"))?;

        // Pack slot 254: address `from` (low 20 bytes) + uint64 srcChainId (next 8 bytes)
        // Solidity packs struct members right-aligned in the same slot:
        //   from occupies bytes [0..20), srcChainId occupies bytes [20..28)
        let mut slot_254 = [0u8; 32];
        slot_254[12..32].copy_from_slice(message_from_l2.from.as_slice());
        slot_254[4..12].copy_from_slice(&message_from_l2.srcChainId.to_be_bytes());
        let slot_254_value = B256::from(slot_254);

        // message_from_l2.data is already the full ABI-encoded calldata for
        // onMessageInvocation(bytes) — exactly what Bridge.processMessage
        // would pass to the target. Use it directly.
        // Forward message.value as msg.value so payable callbacks receive ETH.
        let callback_address = message_from_l2.to;
        let tx_request = TransactionRequest::default()
            .from(bridge_address) // msg.sender = bridge (passes ONLY_BRIDGE check)
            .to(callback_address)
            .value(message_from_l2.value)
            .input(message_from_l2.data.clone().into());

        // State-override the bridge's __ctx storage so context() returns
        // the correct msgHash, from, and srcChainId. Also give the bridge
        // enough ETH balance so the value transfer succeeds in the trace.
        let bridge_balance = message_from_l2
            .value
            .saturating_add(U256::from(10u64).pow(U256::from(18u64)));
        let bridge_ctx_override = AccountOverride::default()
            .with_balance(bridge_balance)
            .with_state_diff([
                (B256::from(U256::from(253u64)), msg_hash), // __ctx.msgHash
                (B256::from(U256::from(254u64)), slot_254_value), // __ctx.from + srcChainId
            ]);
        let mut state_overrides = StateOverride::default();
        state_overrides.insert(bridge_address, bridge_ctx_override);

        let tracer_config = serde_json::json!({"onlyTopCall": false});

        let tracing_options = GethDebugTracingOptions {
            tracer: Some(GethDebugTracerType::BuiltInTracer(
                GethDebugBuiltInTracerType::CallTracer,
            )),
            tracer_config: tracer_config.into(),
            ..Default::default()
        };

        let call_options = GethDebugTracingCallOptions {
            tracing_options,
            state_overrides: Some(state_overrides),
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
                return Err(anyhow!("L1 callback simulation RPC failed: {e}"));
            }
        };

        // Scan the trace for a sendMessage call to the L1 bridge.
        let mut return_msg: Option<Message> = None;

        if let alloy::rpc::types::trace::geth::GethTrace::CallTracer(call_frame) = trace_result
            && let Some((mut msg, caller)) =
                find_send_message_in_call_tree(&call_frame, bridge_address)
        {
            // Patch bridge-assigned fields (from, srcChainId, id)
            msg.from = caller;
            msg.srcChainId = self.common.chain_id();
            // Query nextMessageId for the id the bridge would assign
            let bridge_contract = Bridge::new(bridge_address, self.provider.clone());
            if let Ok(next_id) = bridge_contract.nextMessageId().call().await {
                msg.id = next_id;
            }
            return_msg = Some(msg);
        }

        if let Some(m) = return_msg {
            // Compute the signal slot: keccak256("SIGNAL", L1_chain_id, L1_bridge, msgHash)
            let return_msg_hash: B256 =
                bridge.hashMessage(m.clone()).call().await.map_err(|e| {
                    anyhow!("Failed to call Bridge.hashMessage for return msg: {e}")
                })?;

            let l1_chain_id = self.common.chain_id();
            let mut slot_preimage = Vec::with_capacity(6 + 8 + 20 + 32);
            slot_preimage.extend_from_slice(b"SIGNAL");
            slot_preimage.extend_from_slice(&l1_chain_id.to_be_bytes());
            slot_preimage.extend_from_slice(bridge_address.as_slice());
            slot_preimage.extend_from_slice(return_msg_hash.as_slice());
            let signal_slot: FixedBytes<32> = keccak256(&slot_preimage);

            tracing::info!(
                "L1 callback simulation found return signal: slot={}, destChainId={}",
                signal_slot,
                m.destChainId
            );
            Ok(Some((m, signal_slot)))
        } else {
            tracing::debug!("L1 callback simulation found no sendMessage call in trace");
            Ok(None)
        }
    }
}

/// `Bridge.sendMessage(Message)` selector.
const SEND_MESSAGE_SELECTOR: [u8; 4] = [0x1b, 0xdb, 0x00, 0x37];

/// Recursively search call frames for a CALL to `bridge_address` with the
/// `sendMessage` function selector. Returns the decoded `IBridge.Message`
/// and the caller address (msg.sender of the sendMessage call).
fn find_send_message_in_call_tree(
    frame: &CallFrame,
    bridge_address: Address,
) -> Option<(Message, Address)> {
    use alloy::sol_types::SolCall;

    if let Some(to_addr) = frame.to
        && to_addr == bridge_address
    {
        let input = frame.input.as_ref();
        if input.len() >= 4
            && input[0..4] == SEND_MESSAGE_SELECTOR
            && let Ok(decoded) = Bridge::sendMessageCall::abi_decode_raw(&input[4..])
        {
            return Some((decoded._message, frame.from));
        }
    }

    for sub in &frame.calls {
        if let Some(result) = find_send_message_in_call_tree(sub, bridge_address) {
            return Some(result);
        }
    }

    None
}
