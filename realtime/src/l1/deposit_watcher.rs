use crate::shared_abi::bindings::{
    Bridge::MessageSent,
    SignalService::SignalSent,
};
use alloy::{
    primitives::{Address, FixedBytes},
    providers::{DynProvider, Provider},
    rpc::types::Filter,
    sol_types::SolEvent,
};
use anyhow::Result;
use common::utils::cancellation_token::CancellationToken;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::node::proposal_manager::bridge_handler::L2Call;

/// Polls L1 for bridge deposit events and queues them for L2 processing.
pub struct DepositWatcher {
    provider: DynProvider,
    bridge_address: Address,
    signal_service_address: Address,
    l2_chain_id: u64,
    tx: mpsc::Sender<L2Call>,
    cancel_token: CancellationToken,
}

impl DepositWatcher {
    pub fn new(
        provider: DynProvider,
        bridge_address: Address,
        signal_service_address: Address,
        l2_chain_id: u64,
        tx: mpsc::Sender<L2Call>,
        cancel_token: CancellationToken,
    ) -> Self {
        Self {
            provider,
            bridge_address,
            signal_service_address,
            l2_chain_id,
            tx,
            cancel_token,
        }
    }

    /// Start polling in a background task. Returns the join handle.
    pub fn start(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            if let Err(e) = self.run().await {
                error!("DepositWatcher exited with error: {}", e);
            }
        })
    }

    async fn run(self) -> Result<()> {
        let mut from_block = self
            .provider
            .get_block_number()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get block number: {}", e))?;

        info!(
            "DepositWatcher started: bridge={}, signal_service={}, l2_chain_id={}, from_block={}",
            self.bridge_address, self.signal_service_address, self.l2_chain_id, from_block
        );

        loop {
            if self.cancel_token.is_cancelled() {
                info!("DepositWatcher shutting down");
                return Ok(());
            }

            tokio::time::sleep(tokio::time::Duration::from_secs(6)).await;

            let latest_block = match self.provider.get_block_number().await {
                Ok(n) => n,
                Err(e) => {
                    warn!("DepositWatcher: failed to get block number: {}", e);
                    continue;
                }
            };

            if latest_block < from_block {
                continue;
            }

            match self.scan_range(from_block, latest_block).await {
                Ok(count) => {
                    if count > 0 {
                        info!(
                            "DepositWatcher: found {} deposits in blocks {}..{}",
                            count, from_block, latest_block
                        );
                    }
                    from_block = latest_block + 1;
                }
                Err(e) => {
                    warn!(
                        "DepositWatcher: error scanning blocks {}..{}: {}",
                        from_block, latest_block, e
                    );
                    // Retry same range next iteration
                }
            }
        }
    }

    async fn scan_range(&self, from_block: u64, to_block: u64) -> Result<usize> {
        // Query MessageSent events from the bridge
        let bridge_filter = Filter::new()
            .address(self.bridge_address)
            .event_signature(MessageSent::SIGNATURE_HASH)
            .from_block(from_block)
            .to_block(to_block);

        let bridge_logs = self
            .provider
            .get_logs(&bridge_filter)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get MessageSent logs: {}", e))?;

        if bridge_logs.is_empty() {
            return Ok(0);
        }

        // Query SignalSent events from the signal service in the same range
        let signal_filter = Filter::new()
            .address(self.signal_service_address)
            .event_signature(SignalSent::SIGNATURE_HASH)
            .from_block(from_block)
            .to_block(to_block);

        let signal_logs = self
            .provider
            .get_logs(&signal_filter)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get SignalSent logs: {}", e))?;

        // Index signal slots by block number + tx index for matching
        let mut signal_by_tx: std::collections::HashMap<(u64, u64), FixedBytes<32>> =
            std::collections::HashMap::new();

        for log in &signal_logs {
            if let (Some(block_number), Some(tx_index)) =
                (log.block_number, log.transaction_index)
            {
                let log_data = alloy::primitives::LogData::new_unchecked(
                    log.topics().to_vec(),
                    log.data().data.clone(),
                );
                if let Ok(decoded) = SignalSent::decode_log_data(&log_data) {
                    signal_by_tx.insert((block_number, tx_index), decoded.slot);
                }
            }
        }

        let mut count = 0;

        for log in &bridge_logs {
            let log_data = alloy::primitives::LogData::new_unchecked(
                log.topics().to_vec(),
                log.data().data.clone(),
            );

            let decoded = match MessageSent::decode_log_data(&log_data) {
                Ok(d) => d,
                Err(e) => {
                    warn!("Failed to decode MessageSent: {}", e);
                    continue;
                }
            };

            // Only process messages targeting our L2
            if decoded.message.destChainId != self.l2_chain_id {
                debug!(
                    "Skipping message with destChainId={} (want {})",
                    decoded.message.destChainId, self.l2_chain_id
                );
                continue;
            }

            // Find matching signal slot from the same transaction
            let signal_slot = if let (Some(block_number), Some(tx_index)) =
                (log.block_number, log.transaction_index)
            {
                signal_by_tx.get(&(block_number, tx_index)).copied()
            } else {
                None
            };

            let Some(signal_slot) = signal_slot else {
                warn!(
                    "No matching SignalSent for MessageSent in block={:?} tx={:?}",
                    log.block_number, log.transaction_index
                );
                continue;
            };

            let l2_call = L2Call {
                message_from_l1: decoded.message,
                signal_slot_on_l2: signal_slot,
            };

            if let Err(e) = self.tx.send(l2_call).await {
                error!("Failed to queue deposit L2Call: {}", e);
            } else {
                count += 1;
            }
        }

        Ok(count)
    }
}
