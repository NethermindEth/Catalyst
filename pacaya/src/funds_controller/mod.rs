mod config;

use config::FundsControllerConfig;
use crate::l1::execution_layer::ExecutionLayer as L1ExecutionLayer;
use crate::l2::taiko::Taiko;
use alloy::primitives::U256;
use anyhow::Error;
use std::sync::Arc;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use common::{
    l1::{el_trait::ELTrait, ethereum_l1::EthereumL1},
    metrics::Metrics,
};

pub struct FundsController {
    config: FundsControllerConfig,
    ethereum_l1: Arc<EthereumL1<L1ExecutionLayer>>,
    taiko: Arc<Taiko>,
    metrics: Arc<Metrics>,
    cancel_token: CancellationToken,
}

impl FundsController {
    pub fn new(
        config: FundsControllerConfig,
        ethereum_l1: Arc<EthereumL1<L1ExecutionLayer>>,
        taiko: Arc<Taiko>,
        metrics: Arc<Metrics>,
        cancel_token: CancellationToken,
    ) -> Self {
        Self {
            config,
            ethereum_l1,
            taiko,
            metrics,
            cancel_token,
        }
    }

    pub fn run(self) {
        tokio::spawn(async move {
            info!("Starting funds monitor...");
            self.monitor_funds_level().await;
        });
    }

    async fn monitor_funds_level(self) {
        if let Err(e) = self.check_initial_funds().await {
            error!("{}", e);
            self.cancel_token.cancel();
            return;
        }

        loop {
            self.transfer_funds_from_l2_to_l1_when_needed().await;
            tokio::select! {
                _ = sleep(self.config.monitor_interval) => {},
                _ = self.cancel_token.cancelled() => {
                    info!("Shutdown signal received, exiting metrics loop...");
                    return;
                }
            }
        }
    }

    async fn check_initial_funds(&self) -> Result<(), Error> {
        // Check TAIKO TOKEN balance
        let total_balance = self
            .ethereum_l1
            .execution_layer
            .get_preconfer_total_bonds()
            .await
            .map_err(|e| Error::msg(format!("Failed to fetch bond balance: {e}")))?;

        if total_balance < self.config.thresholds.taiko {
            anyhow::bail!(
                "Total balance ({}) is below the required threshold ({})",
                total_balance,
                self.config.thresholds.taiko
            );
        }

        info!("Preconfer taiko balance are sufficient: {}", total_balance);

        // Check ETH balance
        let balance = self
            .ethereum_l1
            .execution_layer
            .get_preconfer_wallet_eth()
            .await
            .map_err(|e| Error::msg(format!("Failed to fetch ETH balance: {e}")))?;

        if balance < self.config.thresholds.eth {
            anyhow::bail!(
                "ETH balance ({}) is below the required threshold ({})",
                balance,
                self.config.thresholds.eth
            );
        }

        info!("ETH balance is sufficient ({})", balance);

        Ok(())
    }

    async fn transfer_funds_from_l2_to_l1_when_needed(&self) {
        let eth_balance = self
            .ethereum_l1
            .execution_layer
            .get_preconfer_wallet_eth()
            .await;
        let eth_balance_str = match eth_balance.as_ref() {
            Ok(balance) => {
                self.metrics.set_preconfer_eth_balance(*balance);
                balance.to_string()
            }
            Err(e) => {
                warn!("Failed to get preconfer eth balance: {}", e);
                "-".to_string()
            }
        };
        let taiko_balance_str = match self
            .ethereum_l1
            .execution_layer
            .get_preconfer_total_bonds()
            .await
        {
            Ok(balance) => {
                self.metrics.set_preconfer_taiko_balance(balance);
                format!("{balance}")
            }
            Err(e) => {
                warn!("Failed to get preconfer taiko balance: {}", e);
                "-".to_string()
            }
        };

        let preconfer_address = self
            .ethereum_l1
            .execution_layer
            .get_preconfer_alloy_address();

        let l2_eth_balance = self.taiko.get_balance(preconfer_address).await;
        let l2_eth_balance_str = match l2_eth_balance.as_ref() {
            Ok(balance) => {
                self.metrics.set_preconfer_l2_eth_balance(*balance);
                format!("{balance}")
            }
            Err(e) => {
                warn!("Failed to get preconfer l2 eth balance: {}", e);
                "-".to_string()
            }
        };

        info!(
            "Balances - ETH: {}, L2 ETH: {}, TAIKO: {}",
            eth_balance_str, l2_eth_balance_str, taiko_balance_str
        );

        if !self.config.disable_bridging
            && let Ok(l2_eth_balance) = l2_eth_balance
            && l2_eth_balance
                > U256::from(
                    self.config.amount_to_bridge_from_l2_to_l1
                        + u128::from(self.config.bridge_relayer_fee)
                        + u128::from(self.config.bridge_transaction_fee), // estimated transaction fee
                )
        {
            match self
                .taiko
                .transfer_eth_from_l2_to_l1(
                    self.config.amount_to_bridge_from_l2_to_l1,
                    self.config.bridge_relayer_fee,
                )
                .await
            {
                Ok(_) => info!(
                    "Transferred {} ETH from L2 to L1",
                    self.config.amount_to_bridge_from_l2_to_l1
                ),
                Err(e) => warn!("Failed to transfer ETH from L2 to L1: {}", e),
            }
        }
    }
}
