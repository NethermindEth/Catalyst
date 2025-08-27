use super::{
    config::{EthereumL1Config, ProtocolConfig},
    execution_layer_inner::ExecutionLayerInner,
    extension::ELExtension,
    transaction_error::TransactionError,
};
use crate::{metrics::Metrics, shared::alloy_tools, utils::types::*};
use alloy::{
    eips::BlockNumberOrTag,
    primitives::{Address, B256},
    providers::{DynProvider, Provider},
    rpc::types::{Filter, Log},
};
use anyhow::{Error, anyhow};
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tracing::info;

pub struct ExecutionLayer<T: ELExtension> {
    provider: DynProvider,
    pub inner: Arc<ExecutionLayerInner>,
    pub extension: Arc<T>,
    protocol_config: ProtocolConfig,
}

impl<T: ELExtension> ExecutionLayer<T> {
    pub async fn new(
        config_common: EthereumL1Config,
        specific_config: T::Config,
        transaction_error_channel: Sender<TransactionError>,
        metrics: Arc<Metrics>,
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

        let inner = Arc::new(
            ExecutionLayerInner::new(
                provider.clone(),
                preconfer_address,
                config_common.clone(),
                transaction_error_channel,
                metrics,
            )
            .await?,
        );
        let extension = Arc::new(T::new(inner.clone(), provider.clone(), specific_config).await);

        let protocol_config = extension.fetch_protocol_config().await?;

        Ok(Self {
            provider,
            inner,
            extension,
            protocol_config,
        })
    }

    pub fn chain_id(&self) -> u64 {
        self.inner.chain_id()
    }

    pub async fn is_transaction_in_progress(&self) -> Result<bool, Error> {
        self.inner
            .transaction_monitor
            .is_transaction_in_progress()
            .await
    }

    pub async fn get_preconfer_wallet_eth(&self) -> Result<alloy::primitives::U256, Error> {
        let balance = self
            .provider
            .get_balance(self.inner.preconfer_address())
            .await?;
        Ok(balance)
    }

    pub fn get_preconfer_alloy_address(&self) -> Address {
        self.inner.preconfer_address()
    }

    pub fn get_preconfer_address(&self) -> PreconferAddress {
        self.inner.preconfer_address().into_array()
    }

    pub fn get_block_max_gas_limit(&self) -> u32 {
        self.protocol_config.block_max_gas_limit
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

    pub async fn get_preconfer_nonce_latest(&self) -> Result<u64, Error> {
        let nonce_str: String = self
            .provider
            .client()
            .request(
                "eth_getTransactionCount",
                (self.inner.preconfer_address(), "latest"),
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

    pub fn get_config_max_blocks_per_batch(&self) -> u16 {
        self.protocol_config.max_blocks_per_batch
    }

    pub fn get_config_max_anchor_height_offset(&self) -> u64 {
        self.protocol_config.max_anchor_height_offset
    }

    pub fn get_config_block_max_gas_limit(&self) -> u32 {
        self.protocol_config.block_max_gas_limit
    }

    pub fn get_protocol_config(&self) -> ProtocolConfig {
        self.protocol_config.clone()
    }
}

#[cfg(test)]
impl<T: ELExtension> ExecutionLayer<T> {
    pub async fn new_from_pk(
        ws_rpc_url: String,
        private_key: elliptic_curve::SecretKey<k256::Secp256k1>,
        extension: Arc<T>,
    ) -> Result<Self, Error> {
        use super::config::ContractAddresses;
        use crate::metrics::Metrics;
        use crate::shared::signer::Signer;
        use alloy::{
            network::EthereumWallet,
            providers::{ProviderBuilder, WsConnect},
            signers::local::PrivateKeySigner,
        };
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

        Ok(Self {
            provider: provider_ws.clone(),
            protocol_config: ProtocolConfig {
                max_blocks_per_batch: 100,
                max_anchor_height_offset: 100,
                block_max_gas_limit: 1000000000,
                base_fee_config: crate::l1::config::BaseFeeConfig {
                    adjustment_quotient: 100,
                    sharing_pctg: 100,
                    gas_issuance_per_second: 1000000000,
                    min_gas_excess: 1000000000000000000,
                    max_gas_issuance_per_block: 1000000000,
                },
            },
            inner: Arc::new(
                ExecutionLayerInner::new(
                    provider_ws.clone(),
                    preconfer_address.parse()?,
                    ethereum_l1_config,
                    tx_error_sender,
                    metrics,
                )
                .await?,
            ),
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
    use crate::l1::config::BaseFeeConfig;
    use alloy::node_bindings::Anvil;

    struct ExecutionLayerMock;
    impl ELExtension for ExecutionLayerMock {
        type Config = ();
        async fn new(
            _inner: Arc<ExecutionLayerInner>,
            _provider: DynProvider,
            _config: Self::Config,
        ) -> Self {
            Self {}
        }

        async fn fetch_protocol_config(&self) -> Result<ProtocolConfig, Error> {
            Ok(ProtocolConfig {
                max_blocks_per_batch: 100,
                max_anchor_height_offset: 100,
                block_max_gas_limit: 1000000000,
                base_fee_config: BaseFeeConfig {
                    adjustment_quotient: 100,
                    sharing_pctg: 100,
                    gas_issuance_per_second: 1000000000,
                    min_gas_excess: 1000000000000000000,
                    max_gas_issuance_per_block: 1000000000,
                },
            })
        }

        async fn get_preconfer_total_bonds(&self) -> Result<alloy::primitives::U256, Error> {
            Ok(alloy::primitives::U256::from(1000000000000000000u64))
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
