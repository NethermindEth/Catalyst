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
    sol_types::{SolCall, SolEvent},
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
        let raiko_client = specific_config.raiko_client;

        Ok(Self {
            common,
            provider,
            preconfer_address: common_config.signer.get_address(),
            transaction_monitor,
            contract_addresses,
            realtime_inbox,
            raiko_client,
            proof_type,
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

        let builder = ProposalTxBuilder::new(self.provider.clone(), 10, self.proof_type);

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

fn collect_logs_recursive(frame: &CallFrame) -> Vec<CallLogFrame> {
    let mut logs = frame.logs.clone();

    for subcall in &frame.calls {
        logs.extend(collect_logs_recursive(subcall));
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
            let all_logs = collect_logs_recursive(&call_frame);
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
        l2_bridge_address: Address,
    ) -> Result<Option<(Message, FixedBytes<32>)>, anyhow::Error> {
        use alloy::primitives::{B256, U256, keccak256};
        use alloy::rpc::types::state::{AccountOverride, StateOverride};

        // Compute the L2→L1 signal slot that the bridge will check during
        // proveSignalReceived. On L1 SignalService:
        //   slot = keccak256(abi.encodePacked("SIGNAL", srcChainId, app, msgHash))
        // where app = the L2 bridge that emitted the signal.
        //
        // hashMessage = keccak256(abi.encode("TAIKO_MESSAGE", message))
        // Compute it on-chain via Bridge.hashMessage to avoid replicating the
        // exact Solidity abi.encode of a (string, struct) tuple in Rust.

        let bridge = Bridge::new(bridge_address, self.provider.clone());
        let msg_hash: B256 = bridge
            .hashMessage(message_from_l2.clone())
            .call()
            .await
            .map_err(|e| anyhow!("Failed to call Bridge.hashMessage for sim: {e}"))?;

        // Bridge.processMessage on L1 passes `app = resolve(srcChainId,
        // B_BRIDGE) = L2 bridge address` to SignalService.proveSignalReceived.
        // That's what the signal slot was derived from on L2 (msg.sender of
        // SignalService.sendSignal was the L2 bridge). Caller passes in the
        // L2 bridge address (auto-derived from L2 chain id on the L2 side).
        let app = l2_bridge_address;
        let src_chain_id = message_from_l2.srcChainId;

        // Mirror SignalService.getSignalSlot: keccak256(abi.encodePacked(
        //   "SIGNAL", uint64 chainId, address app, bytes32 signal))
        let mut preimage = Vec::with_capacity(6 + 8 + 20 + 32);
        preimage.extend_from_slice(b"SIGNAL");
        preimage.extend_from_slice(&src_chain_id.to_be_bytes());
        preimage.extend_from_slice(app.as_slice());
        preimage.extend_from_slice(msg_hash.as_slice());
        let signal_slot_key: B256 = keccak256(&preimage);

        // Storage slot of `_receivedSignals[signal_slot_key]` on L1 SignalService
        // `_receivedSignals` is at storage slot 253 (see SignalService_Layout.sol).
        let received_signals_base_slot = U256::from(253u64);
        let mut key_preimage = Vec::with_capacity(64);
        key_preimage.extend_from_slice(signal_slot_key.as_slice());
        key_preimage.extend_from_slice(&B256::from(received_signals_base_slot).0);
        let received_signals_storage_slot: B256 = keccak256(&key_preimage);

        // Build calldata for `Bridge.processMessage(message_from_l2, "")` with
        // empty proof. With `_receivedSignals[slot] = true` state-overridden,
        // the fast-signal path in proveSignalReceived succeeds, so the bridge
        // proceeds to invoke the target's onMessageInvocation, whose trace we
        // then scan for the L1→L2 return.
        let calldata = Bridge::processMessageCall {
            _message: message_from_l2,
            _proof: Bytes::new(),
        }
        .abi_encode();

        let tx_request = TransactionRequest::default()
            .from(self.preconfer_address)
            .to(bridge_address)
            .input(calldata.into());

        // State-override: mark the signal as received on the L1 SignalService.
        let signal_service_address = self.contract_addresses.signal_service;
        let account_override = AccountOverride::default().with_state_diff(
            std::iter::once((
                received_signals_storage_slot,
                B256::from(U256::from(1)),
            )),
        );
        let mut state_overrides = StateOverride::default();
        state_overrides.insert(signal_service_address, account_override);

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
                // RPC-level failure (not a revert inside the trace). Surface as error.
                return Err(anyhow!("L1 callback simulation RPC failed: {e}"));
            }
        };

        let mut message: Option<Message> = None;
        let mut slot: Option<FixedBytes<32>> = None;

        if let alloy::rpc::types::trace::geth::GethTrace::CallTracer(call_frame) = trace_result {
            // Collect logs regardless of whether the simulation reverted — the
            // MessageSent event is emitted during processMessage's invoked
            // callback, which may succeed even if a later sub-call reverts.
            let all_logs = collect_logs_recursive(&call_frame);
            for log in all_logs {
                if let Some(topics) = &log.topics
                    && !topics.is_empty()
                {
                    if topics[0] == MessageSent::SIGNATURE_HASH {
                        let log_data = alloy::primitives::LogData::new_unchecked(
                            topics.clone(),
                            log.data.clone().unwrap_or_default(),
                        );
                        let decoded = MessageSent::decode_log_data(&log_data).map_err(|e| {
                            anyhow!("Failed to decode MessageSent from L1 callback sim: {e}")
                        })?;
                        message = Some(decoded.message);
                    } else if topics[0] == SignalSent::SIGNATURE_HASH {
                        let log_data = alloy::primitives::LogData::new_unchecked(
                            topics.clone(),
                            log.data.clone().unwrap_or_default(),
                        );
                        let decoded = SignalSent::decode_log_data(&log_data).map_err(|e| {
                            anyhow!("Failed to decode SignalSent from L1 callback sim: {e}")
                        })?;
                        slot = Some(decoded.slot);
                    }
                }
            }
        }

        if let (Some(m), Some(s)) = (message, slot) {
            tracing::info!(
                "L1 callback simulation found return signal: slot={}, destChainId={}",
                s,
                m.destChainId
            );
            Ok(Some((m, s)))
        } else {
            tracing::debug!(
                "L1 callback simulation produced no MessageSent/SignalSent pair"
            );
            Ok(None)
        }
    }
}
