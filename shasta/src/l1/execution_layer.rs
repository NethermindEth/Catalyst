// TODO remove allow dead_code when the module is used
#![allow(dead_code)]

use crate::l1::config::ContractAddresses;
use alloy::{eips::BlockNumberOrTag, primitives::Address, providers::DynProvider};
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
use pacaya::l1::PreconfOperator;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use common::shared::l2_block::L2Block;
use alloy::primitives::Bytes;

use super::bindings::IInbox;
use super::bindings::IPreconfWhitelist;
use super::proposal_tx_builder::ProposalBuilder;
use super::event_indexer::EventIndexer;

use tracing::{info};

use super::config::EthereumL1Config;

pub struct ExecutionLayer {
    common: ExecutionLayerCommon,
    provider: DynProvider,
    preconfer_address: Address,
    config: EthereumL1Config,
    pub transaction_monitor: TransactionMonitor,
    metrics: Arc<Metrics>,
    extra_gas_percentage: u64,
    contract_addresses: ContractAddresses,
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

        let shasta_inbox = IInbox::new(specific_config.shasta_inbox, provider.clone());
        let shasta_config = shasta_inbox
            .getConfig()
            .call()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to call getConfig for Inbox: {e}"))?;

        let contract_addresses = ContractAddresses {
            shasta_inbox: specific_config.shasta_inbox,
            codec: shasta_config.codec,
            proposer_checker: shasta_config.proposerChecker,
        };

        Ok(Self {
            common,
            provider,
            preconfer_address: common_config.signer.get_address(),
            config: specific_config,
            transaction_monitor,
            metrics,
            extra_gas_percentage: common_config.extra_gas_percentage,
            contract_addresses,
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
    async fn is_operator_for_current_epoch(&self) -> Result<bool, Error> {
        let contract =
            IPreconfWhitelist::new(self.contract_addresses.proposer_checker, &self.provider);
        let operator = contract
            .getOperatorForCurrentEpoch()
            .block(alloy::eips::BlockId::pending())
            .call()
            .await
            .map_err(|e| {
                Error::msg(format!(
                    "Failed to get operator for current epoch: {}, contract: {:?}",
                    e, self.contract_addresses.proposer_checker
                ))
            })?;

        Ok(operator == self.preconfer_address)
    }

    async fn is_operator_for_next_epoch(&self) -> Result<bool, Error> {
        let contract =
            IPreconfWhitelist::new(self.contract_addresses.proposer_checker, &self.provider);
        let operator = contract
            .getOperatorForNextEpoch()
            .block(alloy::eips::BlockId::pending())
            .call()
            .await
            .map_err(|e| {
                Error::msg(format!(
                    "Failed to get operator for next epoch: {}, contract: {:?}",
                    e, self.contract_addresses.proposer_checker
                ))
            })?;
        Ok(operator == self.preconfer_address)
    }

    async fn is_preconf_router_specified_in_taiko_wrapper(&self) -> Result<bool, Error> {
        // TODO verify with actual implementation
        Ok(true)
    }

    async fn get_l2_height_from_taiko_inbox(&self) -> Result<u64, Error> {
        self.get_l2_height_from_taiko_inbox().await
    }

    async fn get_handover_window_slots(&self) -> Result<u64, Error> {
        // TODO verify with actual implementation
        Err(anyhow::anyhow!(
            "Not implemented for Shasta execution layer"
        ))
    }
}

impl ExecutionLayer {
    pub async fn get_l2_height_from_taiko_inbox(&self) -> Result<u64, Error> {
        Ok(1) // TODO Placeholder implementation / get from event indexer
    }

    pub async fn send_batch_to_l1(
        &self,
        l2_blocks: Vec<L2Block>,
        anchor_block_number: u64,
        coinbase: Address,
        num_forced_inclusion: u8,
        event_indexer: Arc<EventIndexer>,
    ) -> Result<(), Error> {

        info!(
            "📦 Proposing with {} blocks | num_forced_inclusion: {}",
            l2_blocks.len(),
            num_forced_inclusion,
        );

        // Build propose transaction
        // TODO fill extra gas percentege from config
        let builder = ProposalBuilder::new(self.provider.clone(), 10);
        let tx = builder
            .build_propose_tx(
                l2_blocks,
                anchor_block_number,
                coinbase,
                self.preconfer_address,
                self.contract_addresses.shasta_inbox,
                Bytes::new(),// TODO fill prover_auth_bytes
                event_indexer,
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
}
