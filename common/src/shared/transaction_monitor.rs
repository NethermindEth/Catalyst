use crate::l1::{config::EthereumL1Config, tools, transaction_error::TransactionError};
use crate::{metrics::Metrics, shared::alloy_tools, signer::Signer};
use alloy::{
    consensus::TxType,
    network::{Network, ReceiptResponse, TransactionBuilder, TransactionBuilder4844},
    primitives::{B256, FixedBytes},
    providers::{
        DynProvider, PendingTransactionBuilder, PendingTransactionError, Provider, RootProvider,
        WatchTxError,
    },
    rpc::types::TransactionRequest,
    transports::TransportErrorKind,
};
use alloy_json_rpc::RpcError;
use anyhow::Error;
use std::future::Future;
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;
use tokio::sync::mpsc::Sender;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

/// Trait for types that can asynchronously build a `TransactionRequest`.
/// Implement this on protocol-specific builders (e.g. `ProposalTxBuilder`)
/// to pass them into `monitor_new_transaction_with_builder`.
pub trait TransactionRequestBuilder: Send + 'static {
    fn build(self) -> impl Future<Output = Result<TransactionRequest, TransactionError>> + Send;
}

// Transaction status enum
#[derive(Debug, Clone, PartialEq)]
pub enum TxStatus {
    Confirmed,
    Failed(String), // Error message
    Pending,
}

/// Receivers returned by `monitor_new_transaction` so the caller can track progress
/// without coupling the monitor's API to sender types.
pub struct TxMonitorHandles {
    pub tx_hash_receiver: tokio::sync::oneshot::Receiver<B256>,
    pub tx_result_receiver: tokio::sync::oneshot::Receiver<bool>,
}

#[derive(Debug, Clone)]
pub struct TransactionMonitorConfig {
    min_priority_fee_per_gas_wei: u128,
    tx_fees_increase_percentage: u128,
    max_attempts_to_send_tx: u64,
    max_attempts_to_wait_tx: u64,
    delay_between_tx_attempts: Duration,
    execution_rpc_urls: Vec<String>,
    signer: Arc<Signer>,
}

pub struct TransactionMonitorThread {
    provider: DynProvider,
    config: TransactionMonitorConfig,
    nonce: u64,
    error_notification_channel: Sender<TransactionError>,
    metrics: Arc<Metrics>,
    chain_id: u64,
    sent_tx_hashes: Vec<FixedBytes<32>>,
    tx_hash_notifier: Option<tokio::sync::oneshot::Sender<B256>>,
    tx_result_notifier: tokio::sync::oneshot::Sender<bool>,
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
    /// Returns handles to receive the tx hash and final result.
    pub async fn monitor_new_transaction(
        &self,
        tx: TransactionRequest,
        nonce: u64,
    ) -> Result<TxMonitorHandles, Error> {
        let mut guard = self.join_handle.lock().await;
        if let Some(join_handle) = guard.as_ref()
            && !join_handle.is_finished()
        {
            return Err(Error::msg(
                "Cannot monitor new transaction, previous transaction is in progress",
            ));
        }

        let (tx_hash_sender, tx_hash_receiver) = tokio::sync::oneshot::channel();
        let (tx_result_sender, tx_result_receiver) = tokio::sync::oneshot::channel();
        let handles = TxMonitorHandles {
            tx_hash_receiver,
            tx_result_receiver,
        };

        let monitor_thread = TransactionMonitorThread::new(
            self.provider.clone(),
            self.config.clone(),
            nonce,
            self.error_notification_channel.clone(),
            self.metrics.clone(),
            self.chain_id,
            tx_hash_sender,
            tx_result_sender,
        );
        let join_handle = monitor_thread.spawn_monitoring_task(tx);
        *guard = Some(join_handle);
        Ok(handles)
    }

    /// Monitor a transaction built by a deferred builder.
    /// The builder future is awaited inside the spawned task, so this method returns immediately.
    /// If the builder fails, the error is sent via the error notification channel.
    pub async fn monitor_new_transaction_with_builder(
        &self,
        tx_builder: impl TransactionRequestBuilder,
        nonce: u64,
    ) -> Result<TxMonitorHandles, Error> {
        let mut guard = self.join_handle.lock().await;
        if let Some(join_handle) = guard.as_ref()
            && !join_handle.is_finished()
        {
            return Err(Error::msg(
                "Cannot monitor new transaction, previous transaction is in progress",
            ));
        }

        let (tx_hash_sender, tx_hash_receiver) = tokio::sync::oneshot::channel();
        let (tx_result_sender, tx_result_receiver) = tokio::sync::oneshot::channel();
        let handles = TxMonitorHandles {
            tx_hash_receiver,
            tx_result_receiver,
        };

        let monitor_thread = TransactionMonitorThread::new(
            self.provider.clone(),
            self.config.clone(),
            nonce,
            self.error_notification_channel.clone(),
            self.metrics.clone(),
            self.chain_id,
            tx_hash_sender,
            tx_result_sender,
        );
        let join_handle = monitor_thread.spawn_monitoring_task_with_builder(tx_builder);
        *guard = Some(join_handle);
        Ok(handles)
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
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider: DynProvider,
        config: TransactionMonitorConfig,
        nonce: u64,
        error_notification_channel: Sender<TransactionError>,
        metrics: Arc<Metrics>,
        chain_id: u64,
        tx_hash_notifier: tokio::sync::oneshot::Sender<B256>,
        tx_result_notifier: tokio::sync::oneshot::Sender<bool>,
    ) -> Self {
        Self {
            provider,
            config,
            nonce,
            error_notification_channel,
            metrics,
            chain_id,
            sent_tx_hashes: Vec::new(),
            tx_hash_notifier: Some(tx_hash_notifier),
            tx_result_notifier,
        }
    }
    pub fn spawn_monitoring_task(self, tx: TransactionRequest) -> JoinHandle<()> {
        tokio::spawn(async move {
            self.monitor_transaction(tx).await;
        })
    }

    fn notify_result(self, success: bool) {
        if let Err(err) = self.tx_result_notifier.send(success) {
            debug!("Transaction result ({err}) signal dropped (receiver not listening)");
        }
    }

    pub fn spawn_monitoring_task_with_builder(
        self,
        tx_builder: impl TransactionRequestBuilder,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            match tx_builder.build().await {
                Ok(tx) => {
                    self.monitor_transaction(tx).await;
                }
                Err(err) => {
                    error!("Transaction builder failed: {}", err);
                    self.send_error_signal(err).await;
                    // notifiers are dropped here, receivers will see channel closed
                }
            }
        })
    }

    async fn monitor_transaction(mut self, mut tx: TransactionRequest) {
        tx.set_nonce(self.nonce);
        if !matches!(tx.buildable_type(), Some(TxType::Eip1559 | TxType::Eip4844)) {
            self.send_error_signal(TransactionError::UnsupportedTransactionType)
                .await;
            self.notify_result(false);
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
                    self.notify_result(false);
                    return;
                }
            };

            if sending_attempt > 0 && self.verify_tx_included(sending_attempt).await {
                self.notify_result(true);
                return;
            }

            let pending_tx =
                if let Some(pending_tx) = self.send_transaction(tx_clone, sending_attempt).await {
                    pending_tx
                } else {
                    self.notify_result(false);
                    return;
                };

            let tx_hash = *pending_tx.tx_hash();
            self.sent_tx_hashes.push(tx_hash);

            // Notify the first tx hash to the caller (fires once, on first send attempt)
            if let Some(notifier) = self.tx_hash_notifier.take() {
                let _ = notifier.send(tx_hash);
            }

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

            if let Some(confirmed) = self
                .is_transaction_handled_by_builder(
                    pending_tx.provider().clone(),
                    tx_hash,
                    l1_block_at_send,
                    sending_attempt,
                )
                .await
            {
                self.notify_result(confirmed);
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
        let mut result: Option<bool> = None;
        if let Some(root_provider) = root_provider {
            // We can use unwrap since tx_hashes is updated before root_provider
            let tx_hash = self
                .sent_tx_hashes
                .last()
                .expect("assert: tx_hashes is updated before root_provider");
            while wait_attempt < self.config.max_attempts_to_wait_tx {
                if let Some(confirmed) = self
                    .is_transaction_handled_by_builder(
                        root_provider.clone(),
                        *tx_hash,
                        l1_block_at_send,
                        self.config.max_attempts_to_send_tx,
                    )
                    .await
                {
                    result = Some(confirmed);
                    break;
                }
                if self
                    .verify_tx_included(wait_attempt + self.config.max_attempts_to_send_tx)
                    .await
                {
                    result = Some(true);
                    break;
                }
                warn!("🟣 Transaction watcher timed out without a result. Waiting...");
                wait_attempt += 1;
            }
        }

        match result {
            Some(confirmed) => self.notify_result(confirmed),
            None => {
                if wait_attempt >= self.config.max_attempts_to_wait_tx {
                    error!(
                        "⛔ Transaction {} with nonce {} not confirmed",
                        self.sent_tx_hashes
                            .last()
                            .map_or_else(|| "unknown".to_string(), |h| h.to_string()),
                        self.nonce,
                    );
                    self.send_error_signal(TransactionError::NotConfirmed).await;
                }
                self.notify_result(false);
            }
        }
    }

    /// Returns Some(true) if confirmed, Some(false) if failed, None if still pending.
    async fn is_transaction_handled_by_builder(
        &self,
        root_provider: RootProvider<alloy::network::Ethereum>,
        tx_hash: B256,
        l1_block_at_send: u64,
        sending_attempt: u64,
    ) -> Option<bool> {
        loop {
            let check_tx = PendingTransactionBuilder::new(root_provider.clone(), tx_hash);
            let tx_status = self.wait_for_tx_receipt(check_tx, sending_attempt).await;
            match tx_status {
                TxStatus::Confirmed => return Some(true),
                TxStatus::Failed(err_str) => {
                    if let Some(error) = tools::convert_error_payload(&err_str) {
                        self.send_error_signal(error).await;
                        return Some(false);
                    }
                    self.send_error_signal(TransactionError::TransactionReverted)
                        .await;
                    return Some(false);
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
                    return Some(false);
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

        None
    }

    async fn send_transaction(
        &self,
        tx: TransactionRequest,
        sending_attempt: u64,
    ) -> Option<PendingTransactionBuilder<alloy::network::Ethereum>> {
        match self.provider.send_transaction(tx.clone()).await {
            Ok(pending_tx) => {
                self.propagate_transaction_to_other_backup_nodes(tx).await;
                Some(pending_tx)
            }
            Err(e) => {
                self.handle_rpc_error(e, sending_attempt).await;
                None
            }
        }
    }

    /// Recreates each backup node every time to avoid connection issues
    async fn propagate_transaction_to_other_backup_nodes(&self, tx: TransactionRequest) {
        // Skip the first RPC URL since it is the main one
        for url in self.config.execution_rpc_urls.iter().skip(1) {
            let provider = alloy_tools::construct_alloy_provider(&self.config.signer, url).await;
            match provider {
                Ok(provider) => {
                    let tx = provider.send_transaction(tx.clone()).await;
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

    async fn handle_rpc_error(&self, e: RpcError<TransportErrorKind>, sending_attempt: u64) {
        if let RpcError::ErrorResp(err) = &e {
            if err.message.contains("nonce too low") {
                if !self.verify_tx_included(sending_attempt).await {
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

    async fn verify_tx_included(&self, sending_attempt: u64) -> bool {
        for tx_hash in self.sent_tx_hashes.iter() {
            let tx = self.provider.get_transaction_by_hash(*tx_hash).await;
            if let Ok(Some(tx)) = tx
                && let Some(block_number) = tx.block_number
            {
                info!(
                    "✅ Transaction {} confirmed in block {} by checking its hash",
                    tx_hash, block_number
                );
                self.metrics.observe_batch_propose_tries(sending_attempt);
                self.metrics.inc_batch_confirmed();
                return true;
            }
        }

        let warning = format!(
            "Transaction not found, checked hashes: {:?}",
            self.sent_tx_hashes
        );
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
                    TxStatus::Confirmed
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
                    if self.verify_tx_included(sending_attempt).await {
                        debug!("Transaction included even though got response from the RPC: {e}");
                        return TxStatus::Confirmed;
                    }
                    error!("Error checking transaction {}: {:?}", tx_hash, e);
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
