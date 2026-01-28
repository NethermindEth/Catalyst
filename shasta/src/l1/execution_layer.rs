use super::config::EthereumL1Config;
use super::proposal_tx_builder::ProposalTxBuilder;
use super::protocol_config::ProtocolConfig;
use crate::node::proposal_manager::proposal::Proposal;
use crate::shared_abi::bindings::{Bridge::MessageSent, IBridge::Message, SignalSent};
use crate::{
    l1::{bindings::UserOpsSubmitter, config::ContractAddresses},
    node::proposal_manager::bridge_handler::UserOpData,
};
use alloy::{
    eips::{BlockId, BlockNumberOrTag},
    primitives::{Address, FixedBytes, U256, aliases::U48},
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
use pacaya::l1::traits::{OperatorError, PreconfOperator, WhitelistProvider};
use std::sync::Arc;
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
    preconfer_address: Address,
    pub transaction_monitor: TransactionMonitor,
    contract_addresses: ContractAddresses,
    inbox_instance: InboxInstance<DynProvider>,
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
        .await?;
        let common = ExecutionLayerCommon::new(provider.clone()).await?;

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

        tracing::info!(
            "Shasta inbox: {}, Proposer checker: {}",
            specific_config.shasta_inbox,
            shasta_config.proposerChecker,
        );
        let contract_addresses = ContractAddresses {
            shasta_inbox: specific_config.shasta_inbox,
            proposer_checker: shasta_config.proposerChecker,
            proposer_multicall: specific_config.proposer_multicall,
            bridge: specific_config.bridge,
        };

        Ok(Self {
            common,
            provider,
            preconfer_address: common_config.signer.get_address(),
            transaction_monitor,
            contract_addresses,
            inbox_instance,
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

    fn get_preconfer_alloy_address(&self) -> Address {
        self.preconfer_address
    }
}

impl PreconfOperator for ExecutionLayer {
    fn get_preconfer_address(&self) -> Address {
        self.preconfer_address
    }

    async fn get_operators_for_current_and_next_epoch(
        &self,
        current_epoch_timestamp: u64,
    ) -> Result<(Address, Address), OperatorError> {
        pacaya::l1::execution_layer::ExecutionLayer::get_operators_for_current_and_next_epoch(
            &self.provider,
            self.contract_addresses.proposer_checker,
            current_epoch_timestamp,
        )
        .await
    }

    async fn is_preconf_router_specified_in_taiko_wrapper(&self) -> Result<bool, Error> {
        // TODO verify with actual implementation
        Ok(true)
    }

    async fn get_l2_height_from_taiko_inbox(&self) -> Result<u64, Error> {
        // TODO
        // Retrieving the L2 height directly from the Inbox is not supported in Shasta.
        // To obtain the L2 height, we need to first fetch the proposal ID using the event indexer.
        // After that, we can call `taiko_lastBlockIdByBatchId` on the L2 Taiko-Geth.
        Ok(0)
    }

    async fn get_handover_window_slots(&self) -> Result<u64, Error> {
        // TODO verify with actual implementation
        // We should return just constant from node config
        Err(anyhow::anyhow!(
            "Not implemented for Shasta execution layer"
        ))
    }
}

impl ExecutionLayer {
    pub async fn send_batch_to_l1(&self, batch: Proposal) -> Result<(), Error> {
        info!(
            "📦 Proposing with {} blocks | num_forced_inclusion: {}",
            batch.l2_blocks.len(),
            batch.num_forced_inclusion,
        );

        // Build propose transaction
        // TODO fill extra gas percentege from config
        let builder =
            ProposalTxBuilder::new(self.provider.clone(), 10, self.checkpoint_signer.clone());

        // Surge: This is now a multicall containing user ops and L1 calls
        let tx = builder
            .build_propose_tx(
                batch,
                self.preconfer_address,
                self.contract_addresses.clone(),
            )
            .await?;

        let pending_nonce = self.get_preconfer_nonce_pending().await?;
        // Spawn a monitor for this transaction
        self.transaction_monitor
            .monitor_new_transaction(tx, pending_nonce)
            .await
            .map_err(|e| Error::msg(format!("Sending batch to L1 failed: {e}")))?;

        Ok(())
    }

    pub async fn is_transaction_in_progress(&self) -> Result<bool, Error> {
        self.transaction_monitor.is_transaction_in_progress().await
    }

    pub async fn fetch_protocol_config(&self) -> Result<ProtocolConfig, Error> {
        let shasta_config = self
            .inbox_instance
            .getConfig()
            .call()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to call getConfig for Inbox: {e}"))?;

        info!(
            "Shasta config: basefeeSharingPctg: {}",
            shasta_config.basefeeSharingPctg,
        );

        Ok(ProtocolConfig::from(&shasta_config))
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
        let state = self.inbox_instance.getCoreState().call().await?;

        Ok(state.nextProposalId.to::<u64>())
    }
}

impl WhitelistProvider for ExecutionLayer {
    async fn is_operator_whitelisted(&self) -> Result<bool, Error> {
        let contract = taiko_bindings::preconf_whitelist::PreconfWhitelist::new(
            self.contract_addresses.proposer_checker,
            &self.provider,
        );
        let operators = contract
            .operators(self.preconfer_address)
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
            .getBond(self.preconfer_address)
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

pub trait L1BridgeHandlerOps {
    // Surge: This can be made to retrieve multiple signal slots
    async fn find_message_and_signal_slot(
        &self,
        user_op_data: UserOpData,
    ) -> Result<Option<(Message, FixedBytes<32>)>, anyhow::Error>;
}

// Surge: Please beward of these limitations
// - Target contracts are not verified in log checks
impl L1BridgeHandlerOps for ExecutionLayer {
    async fn find_message_and_signal_slot(
        &self,
        user_op_data: UserOpData,
    ) -> Result<Option<(Message, FixedBytes<32>)>, anyhow::Error> {
        // Build the call to executeBatch with a single user op
        let submitter = UserOpsSubmitter::new(user_op_data.user_op_submitter, &self.provider);
        let call =
            submitter.executeBatch(vec![user_op_data.user_op], user_op_data.user_op_signature);

        // Create transaction request for simulation
        let tx_request = TransactionRequest::default()
            .from(self.preconfer_address)
            .to(user_op_data.user_op_submitter)
            .input(call.calldata().clone().into());

        // Configure call tracer with logs enabled
        let mut tracer_config = serde_json::Map::new();
        tracer_config.insert("withLog".to_string(), serde_json::Value::Bool(true));

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

        let mut message: Option<Message> = None;
        let mut slot: Option<FixedBytes<32>> = None;

        // Look for logs in the trace result and decode MessageSent and SignalSent events
        if let alloy::rpc::types::trace::geth::GethTrace::CallTracer(call_frame) = trace_result {
            for log in call_frame.logs {
                // Check if this is a MessageSent or SignalSent event by matching the topic
                if let Some(topics) = &log.topics {
                    if !topics.is_empty() {
                        if topics[0] == MessageSent::SIGNATURE_HASH {
                            // Decode the MessageSent event
                            let log_data = alloy::primitives::LogData::new_unchecked(
                                topics.clone(),
                                log.data.clone().unwrap_or_default(),
                            );
                            let decoded = MessageSent::decode_log_data(&log_data).map_err(|e| {
                                anyhow!("Failed to decode MessageSent event L1: {e}")
                            })?;

                            message = Some(decoded.message);
                        } else if topics[0] == SignalSent::SIGNATURE_HASH {
                            // Decode the SignalSent event
                            let log_data = alloy::primitives::LogData::new_unchecked(
                                topics.clone(),
                                log.data.clone().unwrap_or_default(),
                            );
                            let decoded = SignalSent::decode_log_data(&log_data).map_err(|e| {
                                anyhow!("Failed to decode SignalSent event L1: {e}")
                            })?;

                            slot = Some(decoded.slot);
                        }
                    }
                }
            }
        }

        if let (Some(message), Some(slot)) = (message, slot) {
            return Ok(Some((message, slot)));
        }

        Ok(None)
    }
}
