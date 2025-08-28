use super::{
    bindings::{
        BatchParams, BlockParams, PreconfWhitelist,
        forced_inclusion_store::{IForcedInclusionStore, IForcedInclusionStore::ForcedInclusion},
        taiko_inbox, taiko_wrapper,
    },
    propose_batch_builder::ProposeBatchBuilder,
};
use crate::forced_inclusion::ForcedInclusionInfo;
use alloy::{
    primitives::{Address, U256},
    providers::DynProvider,
};
use anyhow::{Error, anyhow};
use common::{
    l1::{
        bindings::IERC20,
        config::{BaseFeeConfig, ContractAddresses, ProtocolConfig},
        el_trait::ELTrait,
        execution_layer::ExecutionLayer as ExecutionLayerCommon,
        transaction_error::TransactionError,
    },
    metrics::Metrics,
    shared::{alloy_tools, l2_block::L2Block, l2_tx_lists::encode_and_compress},
};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc::Sender;
use tracing::{debug, info, warn};

pub struct EthereumL1Config {
    pub contract_addresses: ContractAddresses,
}

pub struct ExecutionLayer {
    common: ExecutionLayerCommon,
    provider: DynProvider,
    config: EthereumL1Config,
    taiko_wrapper_contract: taiko_wrapper::TaikoWrapper::TaikoWrapperInstance<DynProvider>,
}

impl ELTrait for ExecutionLayer {
    type Config = EthereumL1Config;
    async fn new(
        common_config: common::l1::config::EthereumL1Config,
        specific_config: Self::Config,
        transaction_error_channel: Sender<TransactionError>,
        metrics: Arc<Metrics>,
    ) -> Result<Self, Error> {
        let (provider, preconfer_address) = alloy_tools::construct_alloy_provider(
            &common_config.signer,
            common_config
                .execution_rpc_urls
                .first()
                .ok_or_else(|| anyhow!("L1 RPC URL is required"))?,
            common_config.preconfer_address,
        )
        .await?;
        let protocol_config =
            Self::fetch_protocol_config(&specific_config.contract_addresses.taiko_inbox, &provider)
                .await?;
        let common = ExecutionLayerCommon::new(
            common_config,
            transaction_error_channel,
            metrics,
            protocol_config,
            provider.clone(),
            preconfer_address,
        )
        .await?;

        let taiko_wrapper_contract = taiko_wrapper::TaikoWrapper::new(
            specific_config.contract_addresses.taiko_wrapper,
            provider.clone(),
        );

        Ok(Self {
            common,
            provider,
            config: specific_config,
            taiko_wrapper_contract,
        })
    }

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

    fn common(&self) -> &ExecutionLayerCommon {
        &self.common
    }
}

impl ExecutionLayer {
    async fn fetch_protocol_config(
        taiko_inbox_address: &Address,
        provider: &DynProvider,
    ) -> Result<ProtocolConfig, Error> {
        let pacaya_config = Self::fetch_pacaya_config(taiko_inbox_address, provider)
            .await
            .map_err(|e| Error::msg(format!("Failed to fetch pacaya config: {e}")))?;

        Ok(ProtocolConfig {
            base_fee_config: BaseFeeConfig {
                adjustment_quotient: pacaya_config.baseFeeConfig.adjustmentQuotient,
                sharing_pctg: pacaya_config.baseFeeConfig.sharingPctg,
                gas_issuance_per_second: pacaya_config.baseFeeConfig.gasIssuancePerSecond,
                min_gas_excess: pacaya_config.baseFeeConfig.minGasExcess,
                max_gas_issuance_per_block: pacaya_config.baseFeeConfig.maxGasIssuancePerBlock,
            },
            max_blocks_per_batch: pacaya_config.maxBlocksPerBatch,
            max_anchor_height_offset: pacaya_config.maxAnchorHeightOffset,
            block_max_gas_limit: pacaya_config.blockMaxGasLimit,
        })
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
            self.common.metrics.observe_block_tx_count(u64::from(count));

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

        self.common
            .metrics
            .observe_batch_info(blocks.len() as u64, tx_lists_bytes.len() as u64);

        debug!(
            "Proposing batch: current L1 block: {}, last_block_timestamp {}, last_anchor_origin_height {}",
            self.common.get_l1_height().await?,
            last_block_timestamp,
            last_anchor_origin_height
        );

        // Build proposeBatch transaction
        let builder =
            ProposeBatchBuilder::new(self.provider.clone(), self.common.extra_gas_percentage());
        let tx = builder
            .build_propose_batch_tx(
                self.common.preconfer_address(),
                self.config.contract_addresses.preconf_router,
                tx_lists_bytes,
                blocks.clone(),
                last_anchor_origin_height,
                last_block_timestamp,
                coinbase,
                forced_inclusion,
            )
            .await?;

        let pending_nonce = self.common.get_preconfer_nonce_pending().await?;
        // Spawn a monitor for this transaction
        self.common
            .transaction_monitor
            .monitor_new_transaction(tx, pending_nonce)
            .await
            .map_err(|e| Error::msg(format!("Sending batch to L1 failed: {e}")))?;

        Ok(())
    }

    async fn fetch_pacaya_config(
        taiko_inbox_address: &Address,
        provider: &DynProvider,
    ) -> Result<taiko_inbox::ITaikoInbox::Config, Error> {
        let contract = taiko_inbox::ITaikoInbox::new(*taiko_inbox_address, provider);
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

    pub async fn get_preconfer_inbox_bonds(&self) -> Result<alloy::primitives::U256, Error> {
        let contract = taiko_inbox::ITaikoInbox::new(
            self.config.contract_addresses.taiko_inbox,
            &self.provider,
        );
        let bonds_balance = contract
            .bondBalanceOf(self.common.preconfer_address())
            .call()
            .await
            .map_err(|e| Error::msg(format!("Failed to get bonds balance: {e}")))?;
        Ok(bonds_balance)
    }

    pub async fn get_preconfer_wallet_bonds(&self) -> Result<alloy::primitives::U256, Error> {
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
                self.common.preconfer_address(),
                self.config.contract_addresses.taiko_inbox,
            )
            .call()
            .await
            .map_err(|e| Error::msg(format!("Failed to get allowance: {e}")))?;

        let balance = contract
            .balanceOf(self.common.preconfer_address())
            .call()
            .await
            .map_err(|e| Error::msg(format!("Failed to get preconfer balance: {e}")))?;

        Ok(balance.min(allowance))
    }

    async fn get_operator_for_current_epoch(&self) -> Result<Address, Error> {
        let contract = PreconfWhitelist::new(
            self.config.contract_addresses.preconf_whitelist,
            &self.provider,
        );
        let operator = contract
            .getOperatorForCurrentEpoch()
            .block(alloy::eips::BlockId::pending())
            .call()
            .await
            .map_err(|e| {
                Error::msg(format!(
                    "Failed to get operator for current epoch: {}, contract: {:?}",
                    e, self.config.contract_addresses.preconf_whitelist
                ))
            })?;
        Ok(operator)
    }

    async fn get_operator_for_next_epoch(&self) -> Result<Address, Error> {
        let contract = PreconfWhitelist::new(
            self.config.contract_addresses.preconf_whitelist,
            &self.provider,
        );
        let operator = contract
            .getOperatorForNextEpoch()
            .block(alloy::eips::BlockId::pending())
            .call()
            .await
            .map_err(|e| {
                Error::msg(format!(
                    "Failed to get operator for next epoch: {}, contract: {:?}",
                    e, self.config.contract_addresses.preconf_whitelist
                ))
            })?;
        Ok(operator)
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
            self.common.preconfer_address(),
            coinbase,
            last_anchor_origin_height,
            last_l2_block_timestamp,
            info,
        )
    }
}

use std::future::Future;
pub trait PreconfOperator {
    fn is_operator_for_current_epoch(&self) -> impl Future<Output = Result<bool, Error>> + Send;
    fn is_operator_for_next_epoch(&self) -> impl Future<Output = Result<bool, Error>> + Send;
    fn is_preconf_router_specified_in_taiko_wrapper(
        &self,
    ) -> impl Future<Output = Result<bool, Error>> + Send;
    fn get_l2_height_from_taiko_inbox(&self) -> impl Future<Output = Result<u64, Error>> + Send;
}

impl PreconfOperator for ExecutionLayer {
    async fn is_operator_for_current_epoch(&self) -> Result<bool, Error> {
        let operator = self.get_operator_for_current_epoch().await?;
        Ok(operator == self.common.preconfer_address())
    }

    async fn is_operator_for_next_epoch(&self) -> Result<bool, Error> {
        let operator = self.get_operator_for_next_epoch().await?;
        Ok(operator == self.common.preconfer_address())
    }

    async fn is_preconf_router_specified_in_taiko_wrapper(&self) -> Result<bool, Error> {
        let preconf_router = self.taiko_wrapper_contract.preconfRouter().call().await?;
        Ok(preconf_router != Address::ZERO)
    }

    async fn get_l2_height_from_taiko_inbox(&self) -> Result<u64, Error> {
        self.get_l2_height_from_taiko_inbox().await
    }
}
