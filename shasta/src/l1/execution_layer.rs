use super::config::EthereumL1Config;
use super::proposal_tx_builder::ProposalTxBuilder;
use super::protocol_config::ProtocolConfig;
use crate::l1::config::ContractAddresses;
use alloy::{
    eips::BlockNumberOrTag,
    primitives::{Address, U256, aliases::U48},
    providers::{DynProvider, Provider},
    rpc::client::BatchRequest,
    sol_types::SolCall,
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
        l2_block_v2::L2BlockV2, transaction_monitor::TransactionMonitor,
    },
};
use pacaya::l1::{
    operators_cache::{OperatorError, OperatorsCache},
    traits::{PreconfOperator, WhitelistProvider},
};
use serde_json::json;
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
    pub transaction_monitor: TransactionMonitor,
    contract_addresses: ContractAddresses,
    inbox_instance: InboxInstance<DynProvider>,
    operators_cache: OperatorsCache,
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
    }

    async fn get_preconfer_nonce_pending(&self) -> Result<u64, Error> {
        self.common()
            .get_account_nonce(self.common().preconfer_address(), BlockNumberOrTag::Pending)
            .await
    }

    async fn get_preconfer_nonce_latest(&self) -> Result<u64, Error> {
        self.common()
            .get_account_nonce(self.common().preconfer_address(), BlockNumberOrTag::Latest)
            .await
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
    pub async fn send_batch_to_l1(
        &self,
        l2_blocks: Vec<L2BlockV2>,
        num_forced_inclusion: u16,
    ) -> Result<(), Error> {
        info!(
            "📦 Proposing with {} blocks | num_forced_inclusion: {}",
            l2_blocks.len(),
            num_forced_inclusion,
        );

        // Build propose transaction
        // TODO fill extra gas percentege from config
        let builder = ProposalTxBuilder::new(self.provider.clone(), 10);
        let tx = builder
            .build_propose_tx(
                l2_blocks,
                self.common().preconfer_address(),
                self.contract_addresses.shasta_inbox,
                num_forced_inclusion,
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

    pub async fn get_inbox_forced_inclusion_state(
        &self,
        // TODO use struct: next_proposal_id, head, tail
    ) -> Result<(u64, u64, u64), Error> {
        // Use BatchRequest to send all calls in a single RPC request
        // This ensures the load balancer forwards all calls to the same RPC node
        let client = self.provider.client();
        let mut batch = BatchRequest::new(client);

        let core_state_call_params = json!([{
            "to": self.contract_addresses.shasta_inbox,
            // TODO use OnceLock as get_operators_for_current_and_next_epoch
            "data": "0x6aa6a01a", // Inbox::getCoreStateCall::SELECTOR;
        }, "latest"]);
        let core_state_waiter = batch
            .add_call("eth_call", &core_state_call_params)
            .map_err(|e| anyhow::anyhow!("Failed to add core state call to batch: {e}"))?;

        let forced_inclusion_state_call_params = json!([{
            "to": self.contract_addresses.shasta_inbox,
            // TODO use OnceLock as get_operators_for_current_and_next_epoch
            "data": "0x5ccc1718", // Inbox::getForcedInclusionStateCall::SELECTOR;
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

        Ok((
            core_state.nextProposalId.to::<u64>(),
            forced_inclusion_state.head_.to::<u64>(),
            forced_inclusion_state.tail_.to::<u64>(),
        ))
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
