use super::{
    OperatorError, PreconfOperator,
    bindings::{
        BatchParams, BlockParams, PreconfWhitelist,
        forced_inclusion_store::{IForcedInclusionStore, IForcedInclusionStore::ForcedInclusion},
        preconf_router::IPreconfRouter,
        taiko_inbox, taiko_wrapper,
    },
    config::EthereumL1Config,
    propose_batch_builder::ProposeBatchBuilder,
    protocol_config::ProtocolConfig,
    traits::WhitelistProvider,
};
use crate::forced_inclusion::ForcedInclusionInfo;
use alloy::{
    eips::BlockNumberOrTag,
    primitives::{Address, U256},
    providers::{DynProvider, Provider},
    rpc::client::BatchRequest,
    sol_types::SolCall,
};
use anyhow::{Error, anyhow};
use common::l1::traits::PreconferBondProvider;
use common::{
    l1::{
        bindings::IERC20,
        traits::{ELTrait, PreconferProvider},
        transaction_error::TransactionError,
    },
    metrics::Metrics,
    shared::execution_layer::ExecutionLayer as ExecutionLayerCommon,
    shared::transaction_monitor::TransactionMonitor,
    shared::{alloy_tools, l2_block::L2Block, l2_tx_lists::encode_and_compress},
};
use hex;
use serde_json::json;
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc::Sender;
use tracing::{debug, info, warn};

pub struct ExecutionLayer {
    common: ExecutionLayerCommon,
    provider: DynProvider,
    preconfer_address: Address,
    config: EthereumL1Config,
    taiko_wrapper_contract: taiko_wrapper::TaikoWrapper::TaikoWrapperInstance<DynProvider>,
    pub transaction_monitor: TransactionMonitor,
    metrics: Arc<Metrics>,
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
        let common = ExecutionLayerCommon::new(provider.clone()).await?;

        let taiko_wrapper_contract = taiko_wrapper::TaikoWrapper::new(
            specific_config.contract_addresses.taiko_wrapper,
            provider.clone(),
        );

        let transaction_monitor = TransactionMonitor::new(
            provider.clone(),
            &common_config,
            transaction_error_channel,
            metrics.clone(),
            common.chain_id(),
        )
        .await
        .map_err(|e| Error::msg(format!("Failed to create TransactionMonitor: {e}")))?;

        Ok(Self {
            common,
            provider,
            preconfer_address: common_config.signer.get_address(),
            config: specific_config,
            taiko_wrapper_contract,
            transaction_monitor,
            metrics,
            extra_gas_percentage: common_config.extra_gas_percentage,
        })
    }

    fn common(&self) -> &ExecutionLayerCommon {
        &self.common
    }
}

impl PreconferBondProvider for ExecutionLayer {
    async fn get_preconfer_total_bonds(&self) -> Result<alloy::primitives::U256, Error> {
        // Check TAIKO TOKEN balance
        let bond_balance = self
            .get_preconfer_inbox_bonds()
            .await
            .map_err(|e| Error::msg(format!("Failed to fetch bond balance: {e}")))?;

        let wallet_balance = self
            .get_preconfer_wallet_bonds()
            .await
            .map_err(|e| Error::msg(format!("Failed to fetch bond balance: {e}")))?;

        Ok(bond_balance + wallet_balance)
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

impl ExecutionLayer {
    pub async fn fetch_protocol_config(&self) -> Result<ProtocolConfig, Error> {
        let pacaya_config = self
            .fetch_pacaya_config()
            .await
            .map_err(|e| Error::msg(format!("Failed to fetch pacaya config: {e}")))?;

        Ok(ProtocolConfig::from(pacaya_config))
    }

    pub async fn send_batch_to_l1(
        &self,
        l2_blocks: Vec<L2Block>,
        last_anchor_origin_height: u64,
        coinbase: Address,
        current_l1_slot_timestamp: u64,
        forced_inclusion: Option<BatchParams>,
    ) -> Result<(), Error> {
        let last_block_timestamp = l2_blocks
            .last()
            .ok_or(anyhow::anyhow!("No L2 blocks provided"))?
            .timestamp_sec;

        const DELAYED_L1_PROPOSAL_BUFFER: u64 = 4;

        // Check if the last block timestamp is within the delayed L1 proposal buffer
        // we don't propose in this period because there is a chance that the batch will
        // be included in the previous L1 block and we'll get TimestampTooLarge error.
        if current_l1_slot_timestamp < last_block_timestamp
            && SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()
                <= current_l1_slot_timestamp + DELAYED_L1_PROPOSAL_BUFFER
        {
            warn!("Last block timestamp is within the delayed L1 proposal buffer.");
            return Err(anyhow::anyhow!(TransactionError::EstimationTooEarly));
        }

        let mut tx_vec = Vec::new();
        let mut blocks = Vec::new();

        for (i, l2_block) in l2_blocks.iter().enumerate() {
            let count = u16::try_from(l2_block.prebuilt_tx_list.tx_list.len())?;
            tx_vec.extend(l2_block.prebuilt_tx_list.tx_list.clone());

            // Emit metrics for transaction count in this block
            self.metrics.observe_block_tx_count(u64::from(count));

            /* times_shift is the difference in seconds between the current L2 block and the L2 previous block. */
            let time_shift: u8 = if i == 0 {
                /* For first block, we don't have a previous block to compare the timestamp with. */
                0
            } else {
                (l2_block.timestamp_sec - l2_blocks[i - 1].timestamp_sec)
                    .try_into()
                    .map_err(|e| Error::msg(format!("Failed to convert time shift to u8: {e}")))?
            };
            blocks.push(BlockParams {
                numTransactions: count,
                timeShift: time_shift,
                signalSlots: vec![],
            });
        }

        let tx_lists_bytes = encode_and_compress(&tx_vec)?;

        info!(
            "📦 Proposing batch with {} blocks and {} bytes length | forced inclusion: {}",
            blocks.len(),
            tx_lists_bytes.len(),
            forced_inclusion.is_some(),
        );

        self.metrics
            .observe_batch_info(blocks.len() as u64, tx_lists_bytes.len() as u64);

        debug!(
            "Proposing batch: current L1 block: {}, last_block_timestamp {}, last_anchor_origin_height {}",
            self.common.get_latest_block_id().await?,
            last_block_timestamp,
            last_anchor_origin_height
        );

        // Build proposeBatch transaction
        let builder = ProposeBatchBuilder::new(self.provider.clone(), self.extra_gas_percentage);
        let tx = builder
            .build_propose_batch_tx(
                self.preconfer_address,
                self.config.contract_addresses.preconf_router,
                tx_lists_bytes,
                blocks.clone(),
                last_anchor_origin_height,
                last_block_timestamp,
                coinbase,
                forced_inclusion,
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

    async fn fetch_pacaya_config(&self) -> Result<taiko_inbox::ITaikoInbox::Config, Error> {
        let contract = taiko_inbox::ITaikoInbox::new(
            self.config.contract_addresses.taiko_inbox,
            &self.provider,
        );
        let pacaya_config = contract.pacayaConfig().call().await?;

        info!(
            "Pacaya config: chainid {}, maxUnverifiedBatches {}, batchRingBufferSize {}, maxAnchorHeightOffset {}",
            pacaya_config.chainId,
            pacaya_config.maxUnverifiedBatches,
            pacaya_config.batchRingBufferSize,
            pacaya_config.maxAnchorHeightOffset,
        );

        Ok(pacaya_config)
    }

    pub async fn get_l2_height_from_taiko_inbox(&self) -> Result<u64, Error> {
        let contract = taiko_inbox::ITaikoInbox::new(
            self.config.contract_addresses.taiko_inbox,
            self.provider.clone(),
        );
        let num_batches = contract.getStats2().call().await?.numBatches;
        // It is safe because num_batches initial value is 1
        let batch = contract.getBatch(num_batches - 1).call().await?;

        Ok(batch.lastBlockId)
    }

    async fn get_preconfer_inbox_bonds(&self) -> Result<alloy::primitives::U256, Error> {
        let contract = taiko_inbox::ITaikoInbox::new(
            self.config.contract_addresses.taiko_inbox,
            &self.provider,
        );
        let bonds_balance = contract
            .bondBalanceOf(self.preconfer_address)
            .call()
            .await
            .map_err(|e| Error::msg(format!("Failed to get bonds balance: {e}")))?;
        Ok(bonds_balance)
    }

    async fn get_preconfer_wallet_bonds(&self) -> Result<alloy::primitives::U256, Error> {
        let taiko_token = self
            .config
            .contract_addresses
            .taiko_token
            .get_or_try_init(|| async {
                let contract = taiko_inbox::ITaikoInbox::new(
                    self.config.contract_addresses.taiko_inbox,
                    self.provider.clone(),
                );
                let taiko_token = contract
                    .bondToken()
                    .call()
                    .await
                    .map_err(|e| Error::msg(format!("Failed to get bond token: {e}")))?;
                info!("Taiko token address: {}", taiko_token);
                Ok::<Address, Error>(taiko_token)
            })
            .await?;

        let contract = IERC20::new(*taiko_token, &self.provider);
        let allowance = contract
            .allowance(
                self.preconfer_address,
                self.config.contract_addresses.taiko_inbox,
            )
            .call()
            .await
            .map_err(|e| Error::msg(format!("Failed to get allowance: {e}")))?;

        let balance = contract
            .balanceOf(self.preconfer_address)
            .call()
            .await
            .map_err(|e| Error::msg(format!("Failed to get preconfer balance: {e}")))?;

        Ok(balance.min(allowance))
    }

    /// cached as constant since function has no parameters
    fn get_current_epoch_call_data(
        contract: &PreconfWhitelist::PreconfWhitelistInstance<&DynProvider>,
    ) -> &'static [u8] {
        static CALL_DATA: OnceLock<Vec<u8>> = OnceLock::new();
        CALL_DATA.get_or_init(|| {
            let tx_req = contract
                .getOperatorForCurrentEpoch()
                .into_transaction_request();
            tx_req
                .input
                .input
                .as_ref()
                .expect("get_current_epoch_call_data: Failed to get current epoch call data. Check the whitelist contract bindings.")
                .to_vec()
        })
    }

    /// cached as constant since function has no parameters
    fn get_next_epoch_call_data(
        contract: &PreconfWhitelist::PreconfWhitelistInstance<&DynProvider>,
    ) -> &'static [u8] {
        static CALL_DATA: OnceLock<Vec<u8>> = OnceLock::new();
        CALL_DATA.get_or_init(|| {
            let tx_req = contract
                .getOperatorForNextEpoch()
                .into_transaction_request();
            tx_req.input.input.as_ref().expect("get_next_epoch_call_data: Failed to get next epoch call data. Check the whitelist contract bindings.").to_vec()
        })
    }

    pub async fn get_operators_for_current_and_next_epoch(
        provider: &DynProvider,
        whitelist_address: Address,
        current_epoch_timestamp: u64,
    ) -> Result<(Address, Address), OperatorError> {
        let contract = PreconfWhitelist::new(whitelist_address, provider);
        let current_epoch_call_data = Self::get_current_epoch_call_data(&contract);
        let next_epoch_call_data = Self::get_next_epoch_call_data(&contract);

        // Use BatchRequest to send all calls in a single RPC request
        // This ensures the load balancer forwards all calls to the same RPC node
        let client = provider.client();
        let mut batch = BatchRequest::new(client);

        let block_waiter = batch
            .add_call("eth_getBlockByNumber", &("latest", false))
            .map_err(|e| {
                OperatorError::Any(Error::msg(format!(
                    "Failed to add block call to batch: {e}"
                )))
            })?;

        let current_operator_call_params = json!([{
            "to": whitelist_address,
            "data": format!("0x{}", hex::encode(current_epoch_call_data))
        }, "latest"]);
        let current_operator_waiter = batch
            .add_call("eth_call", &current_operator_call_params)
            .map_err(|e| {
                OperatorError::Any(Error::msg(format!(
                    "Failed to add current operator call to batch: {e}"
                )))
            })?;

        let next_operator_call_params = json!([{
            "to": whitelist_address,
            "data": format!("0x{}", hex::encode(next_epoch_call_data))
        }, "latest"]);
        let next_operator_waiter = batch
            .add_call("eth_call", &next_operator_call_params)
            .map_err(|e| {
                OperatorError::Any(Error::msg(format!(
                    "Failed to add next operator call to batch: {e}"
                )))
            })?;

        batch.send().await.map_err(|e| {
            OperatorError::Any(Error::msg(format!("Failed to send batch request: {e}")))
        })?;

        let block_result: serde_json::Value = block_waiter.await.map_err(|e| {
            OperatorError::Any(Error::msg(format!("Failed to get block from batch: {e}")))
        })?;
        let block: alloy::rpc::types::Block = serde_json::from_value(block_result)
            .map_err(|e| OperatorError::Any(Error::msg(format!("Failed to parse block: {e}"))))?;
        let latest_block_timestamp = block.header.timestamp;
        if latest_block_timestamp < current_epoch_timestamp {
            return Err(OperatorError::OperatorCheckTooEarly);
        }

        let current_operator_result: serde_json::Value =
            current_operator_waiter.await.map_err(|e| {
                OperatorError::Any(Error::msg(format!(
                    "Failed to get current operator from batch: {}, contract: {:?}",
                    e, whitelist_address
                )))
            })?;

        let next_operator_result: serde_json::Value = next_operator_waiter.await.map_err(|e| {
            OperatorError::Any(Error::msg(format!(
                "Failed to get next operator from batch: {}, contract: {:?}",
                e, whitelist_address
            )))
        })?;

        let current_operator_bytes = hex::decode(
            current_operator_result
                .as_str()
                .ok_or_else(|| {
                    OperatorError::Any(Error::msg("Invalid current operator result format"))
                })?
                .strip_prefix("0x")
                .unwrap_or_default(),
        )
        .map_err(|e| {
            OperatorError::Any(Error::msg(format!(
                "Failed to decode current operator: {e}"
            )))
        })?;
        let current_operator =
            <PreconfWhitelist::getOperatorForCurrentEpochCall as SolCall>::abi_decode_returns(
                &current_operator_bytes,
            )
            .map_err(|e| {
                OperatorError::Any(Error::msg(format!(
                    "Failed to decode current operator response: {e}"
                )))
            })?;

        let next_operator_bytes = hex::decode(
            next_operator_result
                .as_str()
                .ok_or_else(|| {
                    OperatorError::Any(Error::msg("Invalid next operator result format"))
                })?
                .strip_prefix("0x")
                .unwrap_or_default(),
        )
        .map_err(|e| {
            OperatorError::Any(Error::msg(format!("Failed to decode next operator: {e}")))
        })?;
        let next_operator =
            <PreconfWhitelist::getOperatorForNextEpochCall as SolCall>::abi_decode_returns(
                &next_operator_bytes,
            )
            .map_err(|e| {
                OperatorError::Any(Error::msg(format!(
                    "Failed to decode next operator response: {e}"
                )))
            })?;

        Ok((current_operator, next_operator))
    }

    pub async fn get_forced_inclusion_head(&self) -> Result<u64, Error> {
        let contract = IForcedInclusionStore::new(
            self.config.contract_addresses.forced_inclusion_store,
            self.provider.clone(),
        );
        contract
            .head()
            .call()
            .await
            .map_err(|e| anyhow!("Failed to get forced inclusion head: {}", e))
    }

    pub async fn get_forced_inclusion_tail(&self) -> Result<u64, Error> {
        let contract = IForcedInclusionStore::new(
            self.config.contract_addresses.forced_inclusion_store,
            self.provider.clone(),
        );
        contract
            .tail()
            .call()
            .await
            .map_err(|e| anyhow!("Failed to get forced inclusion tail: {}", e))
    }

    pub async fn get_forced_inclusion(&self, index: u64) -> Result<ForcedInclusion, Error> {
        let contract = IForcedInclusionStore::new(
            self.config.contract_addresses.forced_inclusion_store,
            self.provider.clone(),
        );
        contract
            .getForcedInclusion(U256::from(index))
            .call()
            .await
            .map_err(|e| {
                Error::msg(format!(
                    "Failed to get forced inclusion at index {index}: {e}"
                ))
            })
    }

    pub fn build_forced_inclusion_batch(
        &self,
        coinbase: Address,
        last_anchor_origin_height: u64,
        last_l2_block_timestamp: u64,
        info: &ForcedInclusionInfo,
    ) -> BatchParams {
        ProposeBatchBuilder::build_forced_inclusion_batch(
            self.preconfer_address,
            coinbase,
            last_anchor_origin_height,
            last_l2_block_timestamp,
            info,
        )
    }

    pub async fn get_preconf_router_config(&self) -> Result<IPreconfRouter::Config, Error> {
        let contract = IPreconfRouter::new(
            self.config.contract_addresses.preconf_router,
            self.provider.clone(),
        );
        contract
            .getConfig()
            .call()
            .await
            .map_err(|e| Error::msg(format!("Failed to get preconf router config: {e}")))
    }

    pub async fn is_transaction_in_progress(&self) -> Result<bool, Error> {
        self.transaction_monitor.is_transaction_in_progress().await
    }
}

impl WhitelistProvider for ExecutionLayer {
    async fn is_operator_whitelisted(&self) -> Result<bool, Error> {
        let contract = PreconfWhitelist::new(
            self.config.contract_addresses.preconf_whitelist,
            &self.provider,
        );
        let operators = contract
            .operators(self.preconfer_address)
            .call()
            .await
            .map_err(|e| {
                Error::msg(format!(
                    "Failed to get operators: {}, contract: {:?}",
                    e, self.config.contract_addresses.preconf_whitelist
                ))
            })?;
        Ok(operators.activeSince > 0)
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
        Self::get_operators_for_current_and_next_epoch(
            &self.provider,
            self.config.contract_addresses.preconf_whitelist,
            current_epoch_timestamp,
        )
        .await
    }

    async fn is_preconf_router_specified_in_taiko_wrapper(&self) -> Result<bool, Error> {
        let preconf_router = self
            .taiko_wrapper_contract
            .preconfRouter()
            .call()
            .await
            .map_err(|e| Error::msg(format!("Failed to get preconf router: {e}")))?;
        Ok(preconf_router != Address::ZERO)
    }

    async fn get_l2_height_from_taiko_inbox(&self) -> Result<u64, Error> {
        self.get_l2_height_from_taiko_inbox().await
    }

    async fn get_handover_window_slots(&self) -> Result<u64, Error> {
        match self.get_preconf_router_config().await {
            Ok(router_config) => router_config.handOverSlots.try_into().map_err(|_| {
                anyhow::anyhow!("Failed to convert handOverSlots from preconf router config")
            }),
            Err(e) => Err(anyhow::anyhow!(
                "Failed to get preconf router config: {}",
                e
            )),
        }
    }
}
