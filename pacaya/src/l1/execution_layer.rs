use super::{bindings::taiko_inbox, config::EthereumL1Config, protocol_config::ProtocolConfig};
use alloy::{eips::BlockNumberOrTag, primitives::Address, providers::DynProvider};
use anyhow::{Context, Error, anyhow};
use common::{
    l1::{
        bindings::IERC20,
        traits::{ELTrait, PreconferBondProvider, PreconferProvider},
        transaction_error::TransactionError,
    },
    metrics::Metrics,
    shared::{
        alloy_tools, execution_layer::ExecutionLayer as ExecutionLayerCommon,
        transaction_monitor::TransactionMonitor,
    },
};
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tracing::info;

pub struct ExecutionLayer {
    common: ExecutionLayerCommon,
    provider: DynProvider,
    config: EthereumL1Config,
    pub transaction_monitor: TransactionMonitor,
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

        Ok(Self {
            common,
            provider,
            config: specific_config,
            transaction_monitor,
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

impl ExecutionLayer {
    pub async fn fetch_protocol_config(&self) -> Result<ProtocolConfig, Error> {
        let pacaya_config = self
            .fetch_pacaya_config()
            .await
            .map_err(|e| Error::msg(format!("Failed to fetch pacaya config: {e}")))?;

        Ok(ProtocolConfig::from(pacaya_config))
    }

    async fn fetch_pacaya_config(&self) -> Result<taiko_inbox::ITaikoInbox::Config, Error> {
        let contract = taiko_inbox::ITaikoInbox::new(
            self.config.contract_addresses.taiko_inbox,
            &self.provider,
        );
        let pacaya_config = contract
            .pacayaConfig()
            .call()
            .await
            .context("ITaikoInbox::pacayaConfig")?;

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
        let num_batches = contract
            .getStats2()
            .call()
            .await
            .context("ITaikoInbox::getStats2")?
            .numBatches;
        // It is safe because num_batches initial value is 1
        let batch = contract
            .getBatch(num_batches - 1)
            .call()
            .await
            .context("ITaikoInbox::getBatch")?;

        Ok(batch.lastBlockId)
    }

    async fn get_preconfer_inbox_bonds(&self) -> Result<alloy::primitives::U256, Error> {
        let contract = taiko_inbox::ITaikoInbox::new(
            self.config.contract_addresses.taiko_inbox,
            &self.provider,
        );
        let bonds_balance = contract
            .bondBalanceOf(self.common().preconfer_address())
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
                self.common().preconfer_address(),
                self.config.contract_addresses.taiko_inbox,
            )
            .call()
            .await
            .map_err(|e| Error::msg(format!("Failed to get allowance: {e}")))?;

        let balance = contract
            .balanceOf(self.common().preconfer_address())
            .call()
            .await
            .map_err(|e| Error::msg(format!("Failed to get preconfer balance: {e}")))?;

        Ok(balance.min(allowance))
    }

    pub async fn is_transaction_in_progress(&self) -> Result<bool, Error> {
        self.transaction_monitor
            .is_transaction_in_progress()
            .await
            .context("is_transaction_in_progress")
    }
}
