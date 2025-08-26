use super::{config::EthereumL1Config, tools, transaction_error::TransactionError};
use crate::{
    metrics::Metrics,
    shared::{alloy_tools, signer::Signer},
};
use alloy::{
    consensus::TxType,
    network::{Network, ReceiptResponse, TransactionBuilder, TransactionBuilder4844},
    primitives::{Address, B256},
    providers::{
        DynProvider, PendingTransactionBuilder, PendingTransactionError, Provider, RootProvider,
        WatchTxError,
    },
    rpc::types::TransactionRequest,
    transports::TransportErrorKind,
};
use alloy_json_rpc::RpcError;
use anyhow::Error;
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;
use tokio::sync::mpsc::Sender;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

// Transaction status enum
#[derive(Debug, Clone, PartialEq)]
pub enum TxStatus {
    Confirmed(u64), // Block number
    Failed(String), // Error message
    Pending,
}

#[derive(Debug, Clone)]
pub struct TransactionMonitorConfig {
    min_priority_fee_per_gas_wei: u128,
    tx_fees_increase_percentage: u128,
    max_attempts_to_send_tx: u64,
    max_attempts_to_wait_tx: u64,
    delay_between_tx_attempts: Duration,
    execution_rpc_urls: Vec<String>,
    preconfer_address: Option<Address>,
    signer: Arc<Signer>,
}

pub struct TransactionMonitorThread {
    provider: DynProvider,
    config: TransactionMonitorConfig,
    nonce: u64,
    error_notification_channel: Sender<TransactionError>,
    metrics: Arc<Metrics>,
    chain_id: u64,
}

//#[derive(Debug)]
pub struct TransactionMonitor {
    provider: DynProvider,
    config: TransactionMonitorConfig,
    join_handle: Mutex<Option<JoinHandle<()>>>,
    error_notification_channel: Sender<TransactionError>,
    metrics: Arc<Metrics>,
    chain_id: u64,
}

impl TransactionMonitor {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        provider: DynProvider,
        config: &EthereumL1Config,
        error_notification_channel: Sender<TransactionError>,
        metrics: Arc<Metrics>,
        chain_id: u64,
    ) -> Result<Self, Error> {
        Ok(Self {
            provider,
            config: TransactionMonitorConfig {
                min_priority_fee_per_gas_wei: u128::from(config.min_priority_fee_per_gas_wei),
                tx_fees_increase_percentage: u128::from(config.tx_fees_increase_percentage),
                max_attempts_to_send_tx: config.max_attempts_to_send_tx,
                max_attempts_to_wait_tx: config.max_attempts_to_wait_tx,
                delay_between_tx_attempts: Duration::from_secs(
                    config.delay_between_tx_attempts_sec,
                ),
                execution_rpc_urls: config.execution_rpc_urls.clone(),
                preconfer_address: config.preconfer_address,
                signer: config.signer.clone(),
            },
            join_handle: Mutex::new(None),
            error_notification_channel,
            metrics,
            chain_id,
        })
    }
}

impl TransactionMonitor {
    /// Monitor a transaction until it is confirmed or fails.
    /// Spawns a new tokio task to monitor the transaction.
    pub async fn monitor_new_transaction(
        &self,
        tx: TransactionRequest,
        nonce: u64,
    ) -> Result<(), Error> {
        let mut guard = self.join_handle.lock().await;
        if let Some(join_handle) = guard.as_ref()
            && !join_handle.is_finished()
        {
            return Err(Error::msg(
                "Cannot monitor new transaction, previous transaction is in progress",
            ));
        }

        let monitor_thread = TransactionMonitorThread::new(
            self.provider.clone(),
            self.config.clone(),
            nonce,
            self.error_notification_channel.clone(),
            self.metrics.clone(),
            self.chain_id,
        );
        let join_handle = monitor_thread.spawn_monitoring_task(tx);
        *guard = Some(join_handle);
        Ok(())
    }

    pub async fn is_transaction_in_progress(&self) -> Result<bool, Error> {
        let guard = self.join_handle.lock().await;
        if let Some(join_handle) = guard.as_ref() {
            return Ok(!join_handle.is_finished());
        }
        Ok(false)
    }
}

impl TransactionMonitorThread {
    pub fn new(
        provider: DynProvider,
        config: TransactionMonitorConfig,
        nonce: u64,
        error_notification_channel: Sender<TransactionError>,
        metrics: Arc<Metrics>,
        chain_id: u64,
    ) -> Self {
        Self {
            provider,
            config,
            nonce,
            error_notification_channel,
            metrics,
            chain_id,
        }
    }
    pub fn spawn_monitoring_task(self, tx: TransactionRequest) -> JoinHandle<()> {
        tokio::spawn(async move {
            self.monitor_transaction(tx).await;
        })
    }

    async fn monitor_transaction(&self, mut tx: TransactionRequest) {
        tx.set_nonce(self.nonce);
        if !matches!(tx.buildable_type(), Some(TxType::Eip1559 | TxType::Eip4844)) {
            self.send_error_signal(TransactionError::UnsupportedTransactionType)
                .await;
            return;
        }
        tx.set_chain_id(self.chain_id);

        debug!(
            "Monitoring tx with nonce: {}  max_fee_per_gas: {:?}, max_priority_fee_per_gas: {:?}, max_fee_per_blob_gas: {:?}",
            self.nonce, tx.max_fee_per_gas, tx.max_priority_fee_per_gas, tx.max_fee_per_blob_gas
        );

        // Initial gas tuning
        let mut max_priority_fee_per_gas = tx
            .max_priority_fee_per_gas
            .expect("assert: tx max_priority_fee_per_gas is set");
        let mut max_fee_per_gas = tx
            .max_fee_per_gas
            .expect("assert: tx max_fee_per_gas is set");
        let mut max_fee_per_blob_gas = tx.max_fee_per_blob_gas;

        // increase priority fee by percentage, rest double
        max_fee_per_gas *= 2;
        max_priority_fee_per_gas +=
            max_priority_fee_per_gas * self.config.tx_fees_increase_percentage / 100;
        let min_priority_fee_per_gas = self.config.min_priority_fee_per_gas_wei;
        if let Some(max_fee_per_blob_gas) = &mut max_fee_per_blob_gas {
            *max_fee_per_blob_gas *= 2;
        }

        if max_priority_fee_per_gas < min_priority_fee_per_gas {
            let diff = min_priority_fee_per_gas - max_priority_fee_per_gas;
            max_fee_per_gas += diff;
            max_priority_fee_per_gas += diff;
        }

        let mut root_provider: Option<RootProvider<alloy::network::Ethereum>> = None;
        let mut l1_block_at_send = 0;

        self.metrics.inc_batch_proposed();
        // Sending attempts loop
        let mut tx_hashes = Vec::new();
        for sending_attempt in 0..self.config.max_attempts_to_send_tx {
            let mut tx_clone = tx.clone();
            self.set_tx_parameters(
                &mut tx_clone,
                max_fee_per_gas,
                max_priority_fee_per_gas,
                max_fee_per_blob_gas,
            );

            l1_block_at_send = match self.provider.get_block_number().await {
                Ok(block_number) => block_number,
                Err(e) => {
                    error!("Failed to get L1 block number: {}", e);
                    self.send_error_signal(TransactionError::GetBlockNumberFailed)
                        .await;
                    return;
                }
            };

            if sending_attempt > 0 && self.verify_tx_included(&tx_hashes, sending_attempt).await {
                return;
            }

            let pending_tx = if let Some(pending_tx) = self
                .send_transaction(tx_clone, &tx_hashes, sending_attempt)
                .await
            {
                pending_tx
            } else {
                return;
            };

            let tx_hash = *pending_tx.tx_hash();
            tx_hashes.push(tx_hash);

            if root_provider.is_none() {
                root_provider = Some(pending_tx.provider().clone());
            }

            info!(
                "{} tx nonce: {}, attempt: {}, l1_block: {}, hash: {},  max_fee_per_gas: {}, max_priority_fee_per_gas: {}, max_fee_per_blob_gas: {:?}",
                if sending_attempt == 0 {
                    "🟢 Send"
                } else {
                    "🟡 Replace"
                },
                self.nonce,
                sending_attempt,
                l1_block_at_send,
                tx_hash,
                max_fee_per_gas,
                max_priority_fee_per_gas,
                max_fee_per_blob_gas
            );

            if self
                .is_transaction_handled_by_builder(
                    pending_tx.provider().clone(),
                    tx_hash,
                    l1_block_at_send,
                    sending_attempt,
                )
                .await
            {
                return;
            }

            // increase fees for next attempt
            // replacement requires 100% more for penalty
            max_fee_per_gas += max_fee_per_gas;
            max_priority_fee_per_gas += max_priority_fee_per_gas;
            if let Some(max_fee_per_blob_gas) = &mut max_fee_per_blob_gas {
                *max_fee_per_blob_gas += *max_fee_per_blob_gas;
            }
        }

        //Wait for transaction result
        let mut wait_attempt = 0;
        if let Some(root_provider) = root_provider {
            // We can use unwrap since tx_hashes is updated before root_provider
            let tx_hash = tx_hashes
                .last()
                .expect("assert: tx_hashes is updated before root_provider");
            while wait_attempt < self.config.max_attempts_to_wait_tx
                && !self
                    .is_transaction_handled_by_builder(
                        root_provider.clone(),
                        *tx_hash,
                        l1_block_at_send,
                        self.config.max_attempts_to_send_tx,
                    )
                    .await
                && !self
                    .verify_tx_included(
                        &tx_hashes,
                        wait_attempt + self.config.max_attempts_to_send_tx,
                    )
                    .await
            {
                warn!("🟣 Transaction watcher timed out without a result. Waiting...");
                wait_attempt += 1;
            }
        }

        if wait_attempt >= self.config.max_attempts_to_wait_tx {
            error!(
                "⛔ Transaction {} with nonce {} not confirmed",
                if let Some(tx_hash) = tx_hashes.last() {
                    tx_hash.to_string()
                } else {
                    "unknown".to_string()
                },
                self.nonce,
            );

            self.send_error_signal(TransactionError::NotConfirmed).await;
        }
    }

    /// Returns true if transaction removed from mempool for any reason
    async fn is_transaction_handled_by_builder(
        &self,
        root_provider: RootProvider<alloy::network::Ethereum>,
        tx_hash: B256,
        l1_block_at_send: u64,
        sending_attempt: u64,
    ) -> bool {
        loop {
            let check_tx = PendingTransactionBuilder::new(root_provider.clone(), tx_hash);
            let tx_status = self.wait_for_tx_receipt(check_tx, sending_attempt).await;
            match tx_status {
                TxStatus::Confirmed(_) => return true,
                TxStatus::Failed(err_str) => {
                    if err_str.contains("0x3d32ffdb") {
                        warn!("⚠️ Transaction reverted TimestampTooLarge()");
                        self.send_error_signal(TransactionError::TimestampTooLarge)
                            .await;
                        return true;
                    } else if tools::check_for_insufficient_funds(&err_str) {
                        self.send_error_signal(TransactionError::InsufficientFunds)
                            .await;
                        return true;
                    } else if tools::check_for_reanchor_required(&err_str) {
                        warn!("Reanchor required: {}", err_str);
                        self.send_error_signal(TransactionError::ReanchorRequired)
                            .await;
                        return true;
                    // 0x1e66a770 -> OldestForcedInclusionDue()
                    } else if tools::check_oldest_forced_inclusion_due(&err_str) {
                        warn!("⚠️ Transaction reverted OldestForcedInclusionDue()");
                        self.send_error_signal(TransactionError::OldestForcedInclusionDue)
                            .await;
                        return true;
                    }
                    self.send_error_signal(TransactionError::TransactionReverted)
                        .await;
                    return true;
                }
                TxStatus::Pending => {} // Continue with retry attempts
            }
            // Check if L1 block number has changed since sending the tx
            // If not, check tx again and wait more
            let current_l1_height = match self.provider.get_block_number().await {
                Ok(block_number) => block_number,
                Err(e) => {
                    error!("Failed to get L1 block number: {}", e);
                    self.send_error_signal(TransactionError::GetBlockNumberFailed)
                        .await;
                    return true;
                }
            };
            if current_l1_height != l1_block_at_send {
                break;
            }
            debug!(
                "🟤 Missing block wait more for tx with nonce {}. Current L1 height: {}, L1 height at send: {}",
                self.nonce, current_l1_height, l1_block_at_send
            );
        }

        false
    }

    async fn send_transaction(
        &self,
        tx: TransactionRequest,
        previous_tx_hashes: &Vec<B256>,
        sending_attempt: u64,
    ) -> Option<PendingTransactionBuilder<alloy::network::Ethereum>> {
        match self.provider.send_transaction(tx.clone()).await {
            Ok(pending_tx) => {
                self.propagate_transaction_to_other_backup_nodes(tx).await;
                Some(pending_tx)
            }
            Err(e) => {
                self.handle_rpc_error(e, previous_tx_hashes, sending_attempt)
                    .await;
                None
            }
        }
    }

    /// Recreates each backup node every time to avoid connection issues
    async fn propagate_transaction_to_other_backup_nodes(&self, tx: TransactionRequest) {
        // Skip the first RPC URL since it is the main one
        for url in self.config.execution_rpc_urls.iter().skip(1) {
            let provider = alloy_tools::construct_alloy_provider(
                &self.config.signer,
                url,
                self.config.preconfer_address,
            )
            .await;
            match provider {
                Ok(provider) => {
                    let tx = provider.0.send_transaction(tx.clone()).await;
                    if let Err(e) = tx {
                        if e.to_string().contains("AlreadyKnown")
                            || e.to_string().to_lowercase().contains("already known")
                        {
                            debug!("Transaction already known to backup node {}", url);
                        } else {
                            warn!("Failed to send transaction to backup node {}: {}", url, e);
                        }
                    } else {
                        info!("Transaction sent to backup node {}", url);
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to construct alloy provider for backup node {}: {}",
                        url, e
                    );
                }
            }
        }
    }

    async fn handle_rpc_error(
        &self,
        e: RpcError<TransportErrorKind>,
        previous_tx_hashes: &Vec<B256>,
        sending_attempt: u64,
    ) {
        if let RpcError::ErrorResp(err) = &e {
            if err.message.contains("nonce too low") {
                if !self
                    .verify_tx_included(previous_tx_hashes, sending_attempt)
                    .await
                {
                    self.send_error_signal(TransactionError::TransactionReverted)
                        .await;
                }
                return;
            } else if let Some(error) = tools::convert_error_payload(&err.message) {
                error!("Failed to send transaction: {}", error);
                self.send_error_signal(error).await;
                return;
            }
        }

        // TODO if it is not revert then rebuild rpc client and retry on rpc error
        error!("Failed to send transaction: {}", e);
        self.send_error_signal(TransactionError::TransactionReverted)
            .await;
    }

    async fn send_error_signal(&self, error: TransactionError) {
        if let Err(e) = self.error_notification_channel.send(error).await {
            error!("Failed to send transaction error signal: {}", e);
        }
    }

    async fn verify_tx_included(&self, tx_hashes: &Vec<B256>, sending_attempt: u64) -> bool {
        for tx_hash in tx_hashes {
            let tx = self.provider.get_transaction_by_hash(*tx_hash).await;
            if let Ok(Some(tx)) = tx
                && let Some(block_number) = tx.block_number
            {
                info!(
                    "✅ Transaction {} confirmed in block {} while trying to replace it",
                    tx_hash, block_number
                );
                self.metrics.observe_batch_propose_tries(sending_attempt);
                self.metrics.inc_batch_confirmed();
                return true;
            }
        }

        let warning = format!("Transaction not found, checked hashes: {tx_hashes:?}");
        warn!("{}", warning);
        false
    }

    async fn wait_for_tx_receipt<N: Network>(
        &self,
        pending_tx: PendingTransactionBuilder<N>,
        sending_attempt: u64,
    ) -> TxStatus {
        let tx_hash = *pending_tx.tx_hash();
        let receipt = pending_tx
            .with_timeout(Some(self.config.delay_between_tx_attempts))
            .get_receipt()
            .await;

        match receipt {
            Ok(receipt) => {
                if receipt.status() {
                    let block_number = if let Some(block_number) = receipt.block_number() {
                        block_number
                    } else {
                        warn!("Block number not found for transaction {}", tx_hash);
                        0
                    };

                    info!(
                        "✅ Transaction {} confirmed in block {}",
                        tx_hash, block_number
                    );
                    self.metrics.observe_batch_propose_tries(sending_attempt);
                    self.metrics.inc_batch_confirmed();
                    TxStatus::Confirmed(block_number)
                } else if let Some(block_number) = receipt.block_number() {
                    let revert_reason = crate::shared::alloy_tools::check_for_revert_reason(
                        &self.provider,
                        tx_hash,
                        block_number,
                    )
                    .await;
                    error!("Transaction {} reverted: {}", tx_hash, revert_reason);
                    TxStatus::Failed(revert_reason)
                } else {
                    let error_msg =
                        format!("Transaction {tx_hash} failed, but block number not found");
                    error!("{}", error_msg);
                    TxStatus::Failed(error_msg)
                }
            }
            Err(e) => match e {
                PendingTransactionError::TxWatcher(WatchTxError::Timeout) => {
                    debug!("Transaction watcher timeout");
                    TxStatus::Pending
                }
                _ => {
                    error!("Error checking transaction {}: {}", tx_hash, e);
                    TxStatus::Pending
                }
            },
        }
    }

    fn set_tx_parameters(
        &self,
        tx: &mut TransactionRequest,
        max_fee_per_gas: u128,
        max_priority_fee_per_gas: u128,
        max_fee_per_blob_gas: Option<u128>,
    ) {
        tx.set_max_priority_fee_per_gas(max_priority_fee_per_gas);
        tx.set_max_fee_per_gas(max_fee_per_gas);
        if let Some(max_fee_per_blob_gas) = max_fee_per_blob_gas {
            tx.set_max_fee_per_blob_gas(max_fee_per_blob_gas);
        }
        tx.set_nonce(self.nonce);

        debug!(
            "Tx params, max_fee_per_gas: {:?}, max_priority_fee_per_gas: {:?}, max_fee_per_blob_gas: {:?}, gas limit: {:?}, nonce: {:?}",
            tx.max_fee_per_gas,
            tx.max_priority_fee_per_gas,
            tx.max_fee_per_blob_gas,
            tx.gas,
            tx.nonce,
        );
    }
}
