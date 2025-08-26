use super::{
    config::{ContractAddresses, EthereumL1Config},
    execution_layer_inner::ExecutionLayerInner,
    extension::ELExtension,
    transaction_error::TransactionError,
};
use crate::{
    l1::{
        bindings::*, monitor_transaction::TransactionMonitor,
        propose_batch_builder::ProposeBatchBuilder,
    },
    metrics,
    shared::{alloy_tools, l2_block::L2Block, l2_tx_lists::encode_and_compress},
    utils::types::*,
};
use alloy::{
    eips::BlockNumberOrTag,
    primitives::{Address, B256},
    providers::{DynProvider, Provider},
    rpc::types::{Filter, Log},
};
use anyhow::{Error, anyhow};
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc::Sender;
use tracing::{debug, info, warn};

const DELAYED_L1_PROPOSAL_BUFFER: u64 = 4;

pub struct ExecutionLayer<T: ELExtension> {
    provider: DynProvider,
    preconfer_address: Address,
    contract_addresses: ContractAddresses,
    pacaya_config: taiko_inbox::ITaikoInbox::Config,
    extra_gas_percentage: u64,
    transaction_monitor: TransactionMonitor,
    metrics: Arc<metrics::Metrics>,
    taiko_wrapper_contract: taiko_wrapper::TaikoWrapper::TaikoWrapperInstance<DynProvider>,
    inner: Arc<ExecutionLayerInner>,
    pub extension: Arc<T>,
}

impl<T: ELExtension> ExecutionLayer<T> {
    pub async fn new(
        config_common: EthereumL1Config,
        specific_config: T::Config,
        transaction_error_channel: Sender<TransactionError>,
        metrics: Arc<metrics::Metrics>,
    ) -> Result<Self, Error> {
        let (provider, preconfer_address) = alloy_tools::construct_alloy_provider(
            &config_common.signer,
            config_common
                .execution_rpc_urls
                .first()
                .ok_or_else(|| anyhow!("L1 RPC URL is required"))?,
            config_common.preconfer_address,
        )
        .await?;
        info!("Catalyst node address: {}", preconfer_address);

        let extra_gas_percentage = config_common.extra_gas_percentage;

        let taiko_wrapper_contract = taiko_wrapper::TaikoWrapper::new(
            config_common.contract_addresses.taiko_wrapper,
            provider.clone(),
        );

        let chain_id = provider
            .get_chain_id()
            .await
            .map_err(|e| Error::msg(format!("Failed to get chain ID: {e}")))?;
        info!("L1 Chain ID: {}", chain_id);

        let transaction_monitor = TransactionMonitor::new(
            provider.clone(),
            &config_common,
            transaction_error_channel,
            metrics.clone(),
            chain_id,
        )
        .await
        .map_err(|e| Error::msg(format!("Failed to create TransactionMonitor: {e}")))?;

        let pacaya_config =
            Self::fetch_pacaya_config(&config_common.contract_addresses.taiko_inbox, &provider)
                .await
                .map_err(|e| Error::msg(format!("Failed to fetch pacaya config: {e}")))?;

        let inner = Arc::new(ExecutionLayerInner::new(chain_id, preconfer_address));
        let extension = Arc::new(T::new(inner.clone(), provider.clone(), specific_config));

        Ok(Self {
            provider,
            preconfer_address,
            contract_addresses: config_common.contract_addresses,
            pacaya_config,
            extra_gas_percentage,
            transaction_monitor,
            metrics,
            taiko_wrapper_contract,
            inner,
            extension,
        })
    }

    pub fn chain_id(&self) -> u64 {
        self.inner.chain_id()
    }

    async fn get_operator_for_current_epoch(&self) -> Result<Address, Error> {
        let contract =
            PreconfWhitelist::new(self.contract_addresses.preconf_whitelist, &self.provider);
        let operator = contract
            .getOperatorForCurrentEpoch()
            .block(alloy::eips::BlockId::pending())
            .call()
            .await
            .map_err(|e| {
                Error::msg(format!(
                    "Failed to get operator for current epoch: {}, contract: {:?}",
                    e, self.contract_addresses.preconf_whitelist
                ))
            })?;
        Ok(operator)
    }

    async fn get_operator_for_next_epoch(&self) -> Result<Address, Error> {
        let contract =
            PreconfWhitelist::new(self.contract_addresses.preconf_whitelist, &self.provider);
        let operator = contract
            .getOperatorForNextEpoch()
            .block(alloy::eips::BlockId::pending())
            .call()
            .await
            .map_err(|e| {
                Error::msg(format!(
                    "Failed to get operator for next epoch: {}, contract: {:?}",
                    e, self.contract_addresses.preconf_whitelist
                ))
            })?;
        Ok(operator)
    }

    pub async fn is_transaction_in_progress(&self) -> Result<bool, Error> {
        self.transaction_monitor.is_transaction_in_progress().await
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
            self.get_l1_height().await?,
            last_block_timestamp,
            last_anchor_origin_height
        );

        // Build proposeBatch transaction
        let builder = ProposeBatchBuilder::new(self.provider.clone(), self.extra_gas_percentage);
        let tx = builder
            .build_propose_batch_tx(
                self.preconfer_address,
                self.contract_addresses.preconf_router,
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

    pub fn get_pacaya_config(&self) -> taiko_inbox::ITaikoInbox::Config {
        self.pacaya_config.clone()
    }

    pub async fn get_preconfer_inbox_bonds(&self) -> Result<alloy::primitives::U256, Error> {
        let contract =
            taiko_inbox::ITaikoInbox::new(self.contract_addresses.taiko_inbox, &self.provider);
        let bonds_balance = contract
            .bondBalanceOf(self.preconfer_address)
            .call()
            .await
            .map_err(|e| Error::msg(format!("Failed to get bonds balance: {e}")))?;
        Ok(bonds_balance)
    }

    pub async fn get_preconfer_wallet_bonds(&self) -> Result<alloy::primitives::U256, Error> {
        let taiko_token = self
            .contract_addresses
            .taiko_token
            .get_or_try_init(|| async {
                let contract = taiko_inbox::ITaikoInbox::new(
                    self.contract_addresses.taiko_inbox,
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
            .allowance(self.preconfer_address, self.contract_addresses.taiko_inbox)
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

    pub async fn get_preconfer_total_bonds(&self) -> Result<alloy::primitives::U256, Error> {
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

    pub async fn get_preconfer_wallet_eth(&self) -> Result<alloy::primitives::U256, Error> {
        let balance = self.provider.get_balance(self.preconfer_address).await?;
        Ok(balance)
    }

    pub fn get_config_max_blocks_per_batch(&self) -> u16 {
        self.pacaya_config.maxBlocksPerBatch
    }

    pub fn get_config_max_anchor_height_offset(&self) -> u64 {
        self.pacaya_config.maxAnchorHeightOffset
    }

    pub fn get_config_block_max_gas_limit(&self) -> u32 {
        self.pacaya_config.blockMaxGasLimit
    }

    pub fn get_preconfer_alloy_address(&self) -> Address {
        self.preconfer_address
    }

    pub fn get_preconfer_address(&self) -> PreconferAddress {
        self.preconfer_address.into_array()
    }

    pub async fn get_l1_height(&self) -> Result<u64, Error> {
        self.provider
            .get_block_number()
            .await
            .map_err(|e| Error::msg(format!("Failed to get L1 height: {e}")))
    }

    pub async fn get_block_state_root_by_number(&self, number: u64) -> Result<B256, Error> {
        let block = self
            .provider
            .get_block_by_number(BlockNumberOrTag::Number(number))
            .await
            .map_err(|e| Error::msg(format!("Failed to get block by number ({number}): {e}")))?
            .ok_or(anyhow::anyhow!("Failed to get block by number ({number})"))?;
        Ok(block.header.state_root)
    }

    pub async fn get_l2_height_from_taiko_inbox(&self) -> Result<u64, Error> {
        let contract = taiko_inbox::ITaikoInbox::new(
            self.contract_addresses.taiko_inbox,
            self.provider.clone(),
        );
        let num_batches = contract.getStats2().call().await?.numBatches;
        // It is safe because num_batches initial value is 1
        let batch = contract.getBatch(num_batches - 1).call().await?;

        Ok(batch.lastBlockId)
    }

    pub async fn get_preconfer_nonce_latest(&self) -> Result<u64, Error> {
        let nonce_str: String = self
            .provider
            .client()
            .request(
                "eth_getTransactionCount",
                (self.preconfer_address, "latest"),
            )
            .await
            .map_err(|e| Error::msg(format!("Failed to get nonce: {e}")))?;

        u64::from_str_radix(nonce_str.trim_start_matches("0x"), 16)
            .map_err(|e| Error::msg(format!("Failed to convert nonce: {e}")))
    }

    pub async fn get_preconfer_nonce_pending(&self) -> Result<u64, Error> {
        let nonce_str: String = self
            .provider
            .client()
            .request(
                "eth_getTransactionCount",
                (self.preconfer_address, "pending"),
            )
            .await
            .map_err(|e| Error::msg(format!("Failed to get nonce: {e}")))?;

        u64::from_str_radix(nonce_str.trim_start_matches("0x"), 16)
            .map_err(|e| Error::msg(format!("Failed to convert nonce: {e}")))
    }

    pub async fn get_block_timestamp_by_number(&self, block: u64) -> Result<u64, Error> {
        self.get_block_timestamp_by_number_or_tag(BlockNumberOrTag::Number(block))
            .await
    }

    async fn get_block_timestamp_by_number_or_tag(
        &self,
        block_number_or_tag: BlockNumberOrTag,
    ) -> Result<u64, Error> {
        let block = self
            .provider
            .get_block_by_number(block_number_or_tag)
            .await?
            .ok_or(anyhow::anyhow!(
                "Failed to get block by number ({})",
                block_number_or_tag
            ))?;
        Ok(block.header.timestamp)
    }

    pub async fn get_logs(&self, filter: Filter) -> Result<Vec<Log>, Error> {
        self.provider
            .get_logs(&filter)
            .await
            .map_err(|e| Error::msg(format!("Failed to get logs: {e}")))
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

impl<ELE: ELExtension> PreconfOperator for ExecutionLayer<ELE> {
    async fn is_operator_for_current_epoch(&self) -> Result<bool, Error> {
        let operator = self.get_operator_for_current_epoch().await?;
        Ok(operator == self.preconfer_address)
    }

    async fn is_operator_for_next_epoch(&self) -> Result<bool, Error> {
        let operator = self.get_operator_for_next_epoch().await?;
        Ok(operator == self.preconfer_address)
    }

    async fn is_preconf_router_specified_in_taiko_wrapper(&self) -> Result<bool, Error> {
        let preconf_router = self.taiko_wrapper_contract.preconfRouter().call().await?;
        Ok(preconf_router != Address::ZERO)
    }

    async fn get_l2_height_from_taiko_inbox(&self) -> Result<u64, Error> {
        self.get_l2_height_from_taiko_inbox().await
    }
}

#[cfg(test)]
impl<T: ELExtension> ExecutionLayer<T> {
    pub async fn new_from_pk(
        ws_rpc_url: String,
        private_key: elliptic_curve::SecretKey<k256::Secp256k1>,
        extension: Arc<T>,
    ) -> Result<Self, Error> {
        use super::bindings::taiko_inbox::ITaikoInbox::ForkHeights;
        use crate::metrics::Metrics;
        use crate::shared::signer::Signer;
        use alloy::providers::ProviderBuilder;
        use alloy::providers::WsConnect;
        use alloy::{network::EthereumWallet, signers::local::PrivateKeySigner};
        use tokio::sync::OnceCell;

        let signer = PrivateKeySigner::from_signing_key(private_key.clone().into());
        let wallet = EthereumWallet::from(signer);

        let ws = WsConnect::new(ws_rpc_url.to_string());

        let provider_ws = ProviderBuilder::new()
            .wallet(wallet)
            .connect_ws(ws.clone())
            .await
            .unwrap()
            .erased();

        let preconfer_address = "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"; // some random address for test

        let (tx_error_sender, _) = tokio::sync::mpsc::channel(1);

        let metrics = Arc::new(Metrics::new());

        let ethereum_l1_config = EthereumL1Config {
            execution_rpc_urls: vec![ws_rpc_url],
            contract_addresses: ContractAddresses {
                taiko_inbox: Address::ZERO,
                taiko_token: OnceCell::new(),
                preconf_whitelist: Address::ZERO,
                preconf_router: Address::ZERO,
                taiko_wrapper: Address::ZERO,
                forced_inclusion_store: Address::ZERO,
            },
            consensus_rpc_url: "".to_string(),
            slot_duration_sec: 12,
            slots_per_epoch: 32,
            preconf_heartbeat_ms: 1000,
            signer: Arc::new(Signer::PrivateKey(hex::encode(private_key.to_bytes()))),
            preconfer_address: Some(preconfer_address.parse()?),
            min_priority_fee_per_gas_wei: 1000000000000000000,
            tx_fees_increase_percentage: 5,
            max_attempts_to_send_tx: 4,
            max_attempts_to_wait_tx: 4,
            delay_between_tx_attempts_sec: 15,
            extra_gas_percentage: 5,
        };

        // Self::new(ethereum_l1_config, tx_error_sender, metrics.clone()).await

        Ok(Self {
            provider: provider_ws.clone(),
            preconfer_address: preconfer_address.parse()?,
            contract_addresses: ethereum_l1_config.contract_addresses.clone(),
            pacaya_config: taiko_inbox::ITaikoInbox::Config {
                chainId: 1,
                maxUnverifiedBatches: 100,
                batchRingBufferSize: 100,
                maxBatchesToVerify: 100,
                blockMaxGasLimit: 1000000000,
                livenessBondBase: alloy::primitives::Uint::from_limbs([1000000000000000000, 0]),
                livenessBondPerBlock: alloy::primitives::Uint::from_limbs([1000000000000000000, 0]),
                stateRootSyncInternal: 100,
                maxAnchorHeightOffset: 1000000000000000000,
                baseFeeConfig: taiko_inbox::LibSharedData::BaseFeeConfig {
                    adjustmentQuotient: 100,
                    sharingPctg: 100,
                    gasIssuancePerSecond: 1000000000,
                    minGasExcess: 1000000000000000000,
                    maxGasIssuancePerBlock: 1000000000,
                },
                provingWindow: 1000,
                cooldownWindow: alloy::primitives::Uint::from_limbs([1000000]),
                maxSignalsToReceive: 100,
                maxBlocksPerBatch: 1000,
                forkHeights: ForkHeights {
                    ontake: 0,
                    pacaya: 0,
                    shasta: 0,
                    unzen: 0,
                },
            },
            taiko_wrapper_contract: taiko_wrapper::TaikoWrapper::new(
                Address::ZERO,
                provider_ws.clone(),
            ),
            extra_gas_percentage: 5,
            transaction_monitor: TransactionMonitor::new(
                provider_ws.clone(),
                &ethereum_l1_config,
                tx_error_sender,
                metrics.clone(),
                123456,
            )
            .await
            .unwrap(),
            metrics,
            inner: Arc::new(ExecutionLayerInner::new(1, preconfer_address.parse()?)),
            extension,
        })
    }

    #[cfg(test)]
    async fn call_test_contract(&self) -> Result<(), Error> {
        alloy::sol! {
            #[allow(missing_docs)]
            #[sol(rpc, bytecode="6080806040523460135760df908160198239f35b600080fdfe6080806040526004361015601257600080fd5b60003560e01c9081633fb5c1cb1460925781638381f58a146079575063d09de08a14603c57600080fd5b3460745760003660031901126074576000546000198114605e57600101600055005b634e487b7160e01b600052601160045260246000fd5b600080fd5b3460745760003660031901126074576020906000548152f35b34607457602036600319011260745760043560005500fea2646970667358221220e978270883b7baed10810c4079c941512e93a7ba1cd1108c781d4bc738d9090564736f6c634300081a0033")]
            contract Counter {
                uint256 public number;

                function setNumber(uint256 newNumber) public {
                    number = newNumber;
                }

                function increment() public {
                    number++;
                }
            }
        }

        let contract = Counter::deploy(&self.provider).await?;

        let builder = contract.setNumber(alloy::primitives::U256::from(42));
        let tx_hash = builder.send().await?.watch().await?;
        println!("Set number to 42: {tx_hash}");

        let builder = contract.increment();
        let tx_hash = builder.send().await?.watch().await?;
        println!("Incremented number: {tx_hash}");

        let builder = contract.number();
        let number = builder.call().await?.to_string();

        assert_eq!(number, "43");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::node_bindings::Anvil;

    struct ExecutionLayerMock;
    impl ELExtension for ExecutionLayerMock {
        type Config = ();
        fn new(
            _inner: Arc<ExecutionLayerInner>,
            _provider: DynProvider,
            _config: Self::Config,
        ) -> Self {
            Self {}
        }
    }

    #[tokio::test]
    async fn test_call_contract() {
        // Ensure `anvil` is available in $PATH.
        let anvil = Anvil::new().try_spawn().unwrap();
        let ws_rpc_url = anvil.ws_endpoint();
        let private_key = anvil.keys()[0].clone();
        let el = ExecutionLayer::new_from_pk(ws_rpc_url, private_key, Arc::new(ExecutionLayerMock))
            .await
            .unwrap();
        el.call_test_contract().await.unwrap();
    }
}
