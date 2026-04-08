use super::config::EthereumL1Config;
use super::proposal_tx_builder::ProposalTxBuilder;
use crate::forced_inclusion::InboxForcedInclusionState;
use crate::node::proposal_manager::proposal::Proposal;
use crate::shared_abi::bindings::{Bridge::MessageSent, IBridge::Message, SignalSent};
use crate::{l1::config::ContractAddresses, node::proposal_manager::bridge_handler::UserOp};
use alloy::{
    eips::{BlockId, BlockNumberOrTag},
    hex::ToHexExt,
    primitives::{Address, FixedBytes, U256, aliases::U48},
    providers::{DynProvider, Provider, ext::DebugApi},
    rpc::{
        client::BatchRequest,
        types::{
            TransactionRequest,
            trace::geth::{
                GethDebugBuiltInTracerType, GethDebugTracerType, GethDebugTracingCallOptions,
                GethDebugTracingOptions,
            },
        },
    },
    sol_types::{SolCall, SolEvent},
};
use anyhow::{Context, Error, anyhow};
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
use pacaya::l1::{
    operators_cache::{OperatorError, OperatorsCache},
    traits::{PreconfOperator, WhitelistProvider},
};
use serde_json::json;
use std::sync::{Arc, OnceLock};
use taiko_bindings::inbox::IInbox::Config;
use taiko_bindings::inbox::{
    IForcedInclusionStore::ForcedInclusion,
    IInbox::CoreState,
    Inbox::{self, InboxInstance},
};
use tokio::sync::mpsc::Sender;
use tracing::info;

pub struct ExecutionLayer {
    common: ExecutionLayerCommon,
    provider: DynProvider,
    pub transaction_monitor: TransactionMonitor,
    contract_addresses: ContractAddresses,
    inbox_instance: InboxInstance<DynProvider>,
    operators_cache: OperatorsCache,
    extra_gas_percentage: u64,
    // Surge: For signing the state checkpoints sent as proof with proposal
    checkpoint_signer: alloy::signers::local::PrivateKeySigner,
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
        .await
        .context("construct_alloy_provider")?;
        let common =
            ExecutionLayerCommon::new(provider.clone(), common_config.signer.get_address())
                .await
                .context("ExecutionLayerCommon::new")?;

        let transaction_monitor = TransactionMonitor::new(
            provider.clone(),
            &common_config,
            transaction_error_channel,
            metrics.clone(),
            common.chain_id(),
        )
        .await
        .map_err(|e| Error::msg(format!("Failed to create TransactionMonitor: {e}")))?;

        let inbox_instance = Inbox::new(specific_config.shasta_inbox, provider.clone());
        let shasta_config = inbox_instance
            .getConfig()
            .call()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to call getConfig for Inbox: {e}"))?;

        info!("Shasta config: {:?}", shasta_config);

        let contract_addresses = ContractAddresses {
            shasta_inbox: specific_config.shasta_inbox,
            proposer_checker: shasta_config.proposerChecker,
            proposer_multicall: specific_config.proposer_multicall,
            bridge: specific_config.bridge,
        };

        let operators_cache =
            OperatorsCache::new(provider.clone(), contract_addresses.proposer_checker);

        Ok(Self {
            common,
            provider,
            transaction_monitor,
            contract_addresses,
            inbox_instance,
            operators_cache,
            extra_gas_percentage: common_config.extra_gas_percentage,
            // Surge: Hard coding the private key for the POC
            // (This is the first private key from foundry anvil)
            checkpoint_signer: alloy::signers::local::PrivateKeySigner::from_bytes(
                &"0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
                    .parse::<alloy::primitives::FixedBytes<32>>()?,
            )?,
        })
    }

    fn common(&self) -> &ExecutionLayerCommon {
        &self.common
    }
}

impl PreconferProvider for ExecutionLayer {
    async fn get_preconfer_wallet_eth(&self) -> Result<alloy::primitives::U256, Error> {
        self.common()
            .get_account_balance(self.common().preconfer_address())
            .await
            .context("get_preconfer_wallet_eth")
    }

    async fn get_preconfer_nonce_pending(&self) -> Result<u64, Error> {
        self.common()
            .get_account_nonce(self.common().preconfer_address(), BlockNumberOrTag::Pending)
            .await
            .context("get_preconfer_nonce_pending")
    }

    async fn get_preconfer_nonce_latest(&self) -> Result<u64, Error> {
        self.common()
            .get_account_nonce(self.common().preconfer_address(), BlockNumberOrTag::Latest)
            .await
            .context("get_preconfer_nonce_latest")
    }

    fn get_preconfer_address(&self) -> Address {
        self.common().preconfer_address()
    }
}

impl PreconfOperator for ExecutionLayer {
    fn get_preconfer_address(&self) -> Address {
        self.common().preconfer_address()
    }

    async fn get_operators_for_current_and_next_epoch(
        &self,
        current_epoch_timestamp: u64,
        current_slot_timestamp: u64,
    ) -> Result<(Address, Address), OperatorError> {
        self.operators_cache
            .get_operators_for_current_and_next_epoch(
                current_epoch_timestamp,
                current_slot_timestamp,
            )
            .await
    }

    async fn is_preconf_router_specified_in_taiko_wrapper(&self) -> Result<bool, Error> {
        // Return true for Shasta because we want to skip that check in the operator crate
        Ok(true)
    }

    async fn get_l2_height_from_taiko_inbox(&self) -> Result<u64, Error> {
        // Retrieving the L2 height directly from the Inbox is not supported in Shasta.
        // It requires multiple RPC calls that we want to skip for every heartbeat in Shasta.
        Ok(0)
    }

    async fn get_handover_window_slots(&self) -> Result<u64, Error> {
        // Return a constant value from node config for Shasta
        // since we don't have access to the TaikoWrapper contract in Shasta.
        Err(anyhow::anyhow!(
            "Not implemented for Shasta execution layer"
        ))
    }
}

impl ExecutionLayer {
    pub async fn send_proposal_to_l1(
        &self,
        batch: Proposal,
        tx_hash_notifier: Option<tokio::sync::oneshot::Sender<alloy::primitives::B256>>,
        tx_result_notifier: Option<tokio::sync::oneshot::Sender<bool>>,
    ) -> Result<(), Error> {
        info!(
            "📦 Proposing with {} blocks | num_forced_inclusion: {} | user_ops: {:?} | signal_slots: {:?} | l1_calls: {:?}",
            batch.l2_blocks.len(),
            batch.num_forced_inclusion,
            batch.user_ops,
            batch.signal_slots,
            batch.l1_calls
        );

        let pending_nonce = self.get_preconfer_nonce_pending().await.map_err(|e| {
            Error::msg(format!(
                "get_preconfer_nonce_pending (send_proposal_to_l1) failed: {e}"
            ))
        })?;

        // Build propose transaction
        let builder = ProposalTxBuilder::new(
            self.provider.clone(),
            self.extra_gas_percentage,
            self.checkpoint_signer.clone(),
        );

        // Surge: This is now a multicall containing user ops and L1 calls
        let tx = builder
            .build_propose_tx(
                batch,
                self.common().preconfer_address(),
                self.contract_addresses.clone(),
            )
            .await?;

        self.transaction_monitor
            .monitor_new_transaction(tx, pending_nonce, tx_hash_notifier, tx_result_notifier)
            .await
            .map_err(|e| Error::msg(format!("Sending proposal to L1 failed: {e}")))?;

        Ok(())
    }

    pub async fn is_transaction_in_progress(&self) -> Result<bool, Error> {
        self.transaction_monitor
            .is_transaction_in_progress()
            .await
            .context("is_transaction_in_progress")
    }

    pub async fn fetch_inbox_config(&self) -> Result<Config, Error> {
        self.inbox_instance
            .getConfig()
            .call()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to call getConfig for Inbox: {e}"))
    }

    pub async fn get_activation_timestamp(&self) -> Result<u64, Error> {
        let timestamp = self
            .inbox_instance
            .activationTimestamp()
            .call()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to call activationTimestamp for Inbox: {e}"))?;

        Ok(timestamp.to::<u64>())
    }

    pub async fn get_forced_inclusion_head(&self) -> Result<u64, Error> {
        let state = self
            .inbox_instance
            .getForcedInclusionState()
            .call()
            .await
            .map_err(|e| {
                anyhow::anyhow!("Failed to call getForcedInclusionState for Inbox: {e}")
            })?;

        Ok(state.head_.to::<u64>())
    }

    pub async fn get_forced_inclusion_tail(&self) -> Result<u64, Error> {
        let state = self
            .inbox_instance
            .getForcedInclusionState()
            .call()
            .await
            .map_err(|e| {
                anyhow::anyhow!("Failed to call getForcedInclusionState for Inbox: {e}")
            })?;

        Ok(state.tail_.to::<u64>())
    }

    pub async fn get_forced_inclusion(&self, index: u64) -> Result<ForcedInclusion, Error> {
        let inclusions = self
            .inbox_instance
            .getForcedInclusions(U48::from(index), U48::ONE)
            .call()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to call getForcedInclusions for Inbox: {e}"))?;

        let inclusion = inclusions
            .first()
            .ok_or_else(|| anyhow::anyhow!("No forced inclusion at index {}", index))?;

        Ok(inclusion.clone())
    }

    pub async fn get_inbox_state(&self) -> Result<CoreState, Error> {
        let state = self
            .inbox_instance
            .getCoreState()
            .call()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to call getInboxState for Inbox: {e}"))?;

        Ok(state)
    }

    pub async fn get_inbox_next_proposal_id(&self) -> Result<u64, Error> {
        let state = self
            .inbox_instance
            .getCoreState()
            .call()
            .await
            .context("getCoreState (get_inbox_next_proposal_id)")?;

        Ok(state.nextProposalId.to::<u64>())
    }

    pub async fn get_inbox_forced_inclusion_state(
        &self,
    ) -> Result<InboxForcedInclusionState, Error> {
        // Use BatchRequest to send all calls in a single RPC request
        // This ensures the load balancer forwards all calls to the same RPC node
        let client = self.provider.client();
        let mut batch = BatchRequest::new(client);

        let core_state_calldata = self.get_core_state_calldata(&self.inbox_instance);
        let core_state_call_params = json!([{
            "to": self.contract_addresses.shasta_inbox,
            "data": core_state_calldata, // Inbox::getCoreStateCall
        }, "latest"]);
        let core_state_waiter = batch
            .add_call("eth_call", &core_state_call_params)
            .map_err(|e| anyhow::anyhow!("Failed to add core state call to batch: {e}"))?;

        let forced_inclusion_state_calldata =
            self.get_forced_inclusion_state_calldata(&self.inbox_instance);
        let forced_inclusion_state_call_params = json!([{
            "to": self.contract_addresses.shasta_inbox,
            "data": forced_inclusion_state_calldata, // Inbox::getForcedInclusionStateCall
        }, "latest"]);
        let forced_inclusion_state_waiter = batch
            .add_call("eth_call", &forced_inclusion_state_call_params)
            .map_err(|e| {
                anyhow::anyhow!("Failed to add forced inclusion state call to batch: {e}")
            })?;

        batch
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send batch request: {e}"))?;

        let core_state_result: serde_json::Value = core_state_waiter
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get core state from batch: {e}"))?;
        let forced_inclusion_state_result: serde_json::Value = forced_inclusion_state_waiter
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get forced inclusion state from batch: {e}"))?;

        let core_state_bytes = hex::decode(
            core_state_result
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Invalid core state result format"))?
                .strip_prefix("0x")
                .unwrap_or_default(),
        )
        .map_err(|e| anyhow::anyhow!("Failed to decode core state: {e}"))?;

        let core_state =
            <Inbox::getCoreStateCall as SolCall>::abi_decode_returns(&core_state_bytes)
                .map_err(|e| anyhow::anyhow!("Failed to decode core state response: {e}"))?;

        let forced_inclusion_state_bytes = hex::decode(
            forced_inclusion_state_result
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Invalid forced inclusion state result format"))?
                .strip_prefix("0x")
                .unwrap_or_default(),
        )
        .map_err(|e| anyhow::anyhow!("Failed to decode forced inclusion state: {e}"))?;

        let forced_inclusion_state =
            <Inbox::getForcedInclusionStateCall as SolCall>::abi_decode_returns(
                &forced_inclusion_state_bytes,
            )
            .map_err(|e| {
                anyhow::anyhow!("Failed to decode forced inclusion state response: {e}")
            })?;

        Ok(InboxForcedInclusionState {
            next_proposal_id: core_state.nextProposalId.to::<u64>(),
            head: forced_inclusion_state.head_.to::<u64>(),
            tail: forced_inclusion_state.tail_.to::<u64>(),
        })
    }

    fn get_core_state_calldata(&self, inbox: &InboxInstance<DynProvider>) -> &'static str {
        static CALLDATA: OnceLock<String> = OnceLock::new();
        CALLDATA.get_or_init(|| {
            let tx_req = inbox.getCoreState().into_transaction_request();
            let hex_string = tx_req
                .input
                .input
                .as_ref()
                .expect("get_core_state_calldata: Failed to get core state calldata")
                .to_vec()
                .encode_hex();
            format!("0x{}", hex_string)
        })
    }

    fn get_forced_inclusion_state_calldata(
        &self,
        inbox: &InboxInstance<DynProvider>,
    ) -> &'static str {
        static CALLDATA: OnceLock<String> = OnceLock::new();
        CALLDATA.get_or_init(|| {
            let tx_req = inbox.getForcedInclusionState().into_transaction_request();
            let hex_string = tx_req
                .input
                .input
                .as_ref()
                .expect("get_forced_inclusion_state_calldata: Failed to get forced inclusion state calldata")
                .to_vec()
                .encode_hex();
            format!("0x{}", hex_string)
        })
    }
}

impl WhitelistProvider for ExecutionLayer {
    async fn is_operator_whitelisted(&self) -> Result<bool, Error> {
        let contract = taiko_bindings::preconf_whitelist::PreconfWhitelist::new(
            self.contract_addresses.proposer_checker,
            &self.provider,
        );
        let operators = contract
            .operators(self.common().preconfer_address())
            .call()
            .await
            .map_err(|e| {
                Error::msg(format!(
                    "Failed to get operators: {}, contract: {:?}",
                    e, self.contract_addresses.proposer_checker
                ))
            })?;

        Ok(operators.activeSince > 0)
    }
}

impl common::l1::traits::PreconferBondProvider for ExecutionLayer {
    async fn get_preconfer_total_bonds(&self) -> Result<U256, Error> {
        let bond = self
            .inbox_instance
            .getBond(self.common().preconfer_address())
            .call()
            .await
            .map_err(|e| {
                Error::msg(format!(
                    "Failed to get bond value for the preconfer from inbox: {e}",
                ))
            })?;

        Ok(U256::from(bond.balance))
    }
}

// Surge: L1 EL ops for Bridge Handler

use alloy::rpc::types::trace::geth::{CallFrame, CallLogFrame};

/// Recursively collects all logs from a call frame and its nested subcalls.
/// Logs emitted by nested contract calls are stored within their respective CallFrame objects,
/// not at the top level, so we need to traverse the entire call tree.
fn collect_logs_recursive(frame: &CallFrame) -> Vec<CallLogFrame> {
    let mut logs = frame.logs.clone();

    for subcall in &frame.calls {
        logs.extend(collect_logs_recursive(subcall));
    }

    logs
}

#[allow(async_fn_in_trait)]
pub trait L1BridgeHandlerOps {
    // Surge: This can be made to retrieve multiple signal slots
    async fn find_message_and_signal_slot(
        &self,
        user_op: UserOp,
    ) -> Result<Option<(Message, FixedBytes<32>)>, anyhow::Error>;
}

// Surge: Please beward of these limitations
// - Target contracts are not verified in log checks
impl L1BridgeHandlerOps for ExecutionLayer {
    async fn find_message_and_signal_slot(
        &self,
        user_op_data: UserOp,
    ) -> Result<Option<(Message, FixedBytes<32>)>, anyhow::Error> {
        // Create transaction request for simulation, sending calldata directly to the submitter
        let tx_request = TransactionRequest::default()
            .from(self.common().preconfer_address())
            .to(user_op_data.submitter)
            .input(user_op_data.calldata.into());

        // Configure call tracer with logs and nested calls enabled
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

        // Execute the trace call simulation
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

        // Look for logs in the trace result and decode MessageSent and SignalSent events
        // Note: Logs from nested calls (subcalls) are stored within those nested CallFrame objects,
        // so we need to recursively collect all logs from the entire call tree
        if let alloy::rpc::types::trace::geth::GethTrace::CallTracer(call_frame) = trace_result {
            let all_logs = collect_logs_recursive(&call_frame);
            tracing::debug!("Collected {} logs from call trace", all_logs.len());

            for log in all_logs {
                // Check if this is a MessageSent or SignalSent event by matching the topic
                if let Some(topics) = &log.topics
                    && !topics.is_empty()
                {
                    if topics[0] == MessageSent::SIGNATURE_HASH {
                        // Decode the MessageSent event
                        let log_data = alloy::primitives::LogData::new_unchecked(
                            topics.clone(),
                            log.data.clone().unwrap_or_default(),
                        );
                        let decoded = MessageSent::decode_log_data(&log_data)
                            .map_err(|e| anyhow!("Failed to decode MessageSent event L1: {e}"))?;

                        message = Some(decoded.message);
                    } else if topics[0] == SignalSent::SIGNATURE_HASH {
                        // Decode the SignalSent event
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
}
