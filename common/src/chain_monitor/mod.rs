use crate::utils::cancellation_token::CancellationToken;
use alloy::primitives::{Address, B256};
use alloy::sol_types::SolEvent;
use anyhow::Error;
use batch_proposed_receiver::EventReceiver;
use l2_block_receiver::{L2BlockInfo, L2BlockReceiver};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::mpsc::{self, Receiver};
use tracing::{debug, info};

mod batch_proposed_receiver;
mod l2_block_receiver;

const MESSAGE_QUEUE_SIZE: usize = 20;

struct TaikoGethStatus {
    height: u64,
    hash: B256,
    expected_reorg: Option<u64>,
}

pub struct ChainMonitor<T>
where
    T: SolEvent + Send + 'static,
{
    ws_l1_rpc_url: String,
    ws_l2_rpc_url: String,
    contract: Address,
    taiko_geth_status: Arc<Mutex<TaikoGethStatus>>,
    cancel_token: CancellationToken,
    event_name: &'static str,
    event_handler: fn(&T),
}

impl<T> ChainMonitor<T>
where
    T: SolEvent + Send + 'static,
{
    pub fn new(
        ws_l1_rpc_url: String,
        ws_l2_rpc_url: String,
        contract: Address,
        cancel_token: CancellationToken,
        event_name: &'static str,
        event_handler: fn(&T),
    ) -> Result<Self, Error> {
        debug!(
            "Creating ChainMonitor (L1: {}, L2: {}, Contract: {}, Event : {})",
            ws_l1_rpc_url, ws_l2_rpc_url, contract, event_name
        );

        let taiko_geth_status = Arc::new(Mutex::new(TaikoGethStatus {
            height: 0,
            hash: B256::ZERO,
            expected_reorg: None,
        }));
        Ok(Self {
            ws_l1_rpc_url,
            ws_l2_rpc_url,
            contract,
            taiko_geth_status,
            cancel_token,
            event_name,
            event_handler,
        })
    }

    pub async fn set_expected_reorg(&self, expected_block_number: u64) {
        let mut status = self.taiko_geth_status.lock().await;
        status.expected_reorg = Some(expected_block_number);
    }

    /// Spawns the event listeners and the message handler.
    pub async fn start(&self) -> Result<(), Error> {
        debug!("Starting ChainMonitor");

        //Generic events
        let (event_tx, event_rx) = mpsc::channel(MESSAGE_QUEUE_SIZE);
        let event_receiver = EventReceiver::new(
            self.ws_l1_rpc_url.clone(),
            self.contract,
            event_tx,
            self.cancel_token.clone(),
            self.event_name,
        )
        .await?;
        event_receiver.start();

        //L2 block headers
        let (l2_block_tx, l2_block_rx) = mpsc::channel(MESSAGE_QUEUE_SIZE);
        let l2_receiver = L2BlockReceiver::new(
            self.ws_l2_rpc_url.clone(),
            l2_block_tx,
            self.cancel_token.clone(),
        );
        l2_receiver.start()?;

        let taiko_geth_status = self.taiko_geth_status.clone();
        let cancel_token = self.cancel_token.clone();

        //Message dispatcher
        tokio::spawn(Self::handle_incoming_messages(
            event_rx,
            l2_block_rx,
            taiko_geth_status,
            cancel_token,
            self.event_handler,
        ));

        Ok(())
    }

    async fn handle_incoming_messages(
        mut event_rx: Receiver<T>,
        mut l2_block_rx: Receiver<L2BlockInfo>,
        taiko_geth_status: Arc<Mutex<TaikoGethStatus>>,
        cancel_token: CancellationToken,
        event_handler: fn(&T),
    ) {
        info!("ChainMonitor message loop running");

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    info!("ChainMonitor: cancellation received, shutting down message loop");
                    break;
                }
                Some(event) = event_rx.recv() => {
                    event_handler(&event);
                }
                Some(block) = l2_block_rx.recv() => {
                    info!(
                        "L2 block → number: {}, hash: {}, parent hash: {}",
                        block.block_number, block.block_hash, block.parent_hash,
                    );
                    {
                        let mut status = taiko_geth_status.lock().await;

                        if status.height != 0 && (block.block_number != status.height + 1 || block.parent_hash != status.hash) {
                            let reorg_expected = match status.expected_reorg {
                                Some(expected) => block.block_number == expected,
                                None => false,
                            };
                            if reorg_expected {
                                tracing::debug!("Geth reorg detected: Received L2 block with expected number. Expected: block_id {} hash {}", status.height, status.hash);
                            } else {
                                tracing::warn!("⛔ Geth reorg detected: Received L2 block with unexpected number. Expected: block_id {} hash {}", status.height, status.hash);
                            }
                        }

                        status.height = block.block_number;
                        status.hash = block.block_hash;
                    }

                }
            }
        }
    }
}
