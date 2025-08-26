use alloy::primitives::{Address, B256};
use anyhow::{Error, anyhow};
use batch_proposed_receiver::BatchProposedEventReceiver;
use l2_block_receiver::{L2BlockInfo, L2BlockReceiver};
use std::{str::FromStr, sync::Arc};
use tokio::sync::Mutex;
use tokio::sync::mpsc::{self, Receiver};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

use crate::l1::bindings::taiko_inbox::ITaikoInbox;

mod batch_proposed_receiver;
mod l2_block_receiver;

const MESSAGE_QUEUE_SIZE: usize = 20;

struct TaikoGethStatus {
    height: u64,
    hash: B256,
    expected_reorg: Option<u64>,
}

pub struct ChainMonitor {
    ws_l1_rpc_url: String,
    ws_l2_rpc_url: String,
    taiko_inbox: Address,
    taiko_geth_status: Arc<Mutex<TaikoGethStatus>>,
    cancel_token: CancellationToken,
}

impl ChainMonitor {
    pub fn new(
        ws_l1_rpc_url: String,
        ws_l2_rpc_url: String,
        taiko_inbox: String,
        cancel_token: CancellationToken,
    ) -> Result<Self, Error> {
        debug!(
            "Creating ChainMonitor (L1: {}, L2: {}, Inbox: {})",
            ws_l1_rpc_url, ws_l2_rpc_url, taiko_inbox
        );

        let taiko_inbox = Address::from_str(&taiko_inbox)
            .map_err(|e| anyhow!("Invalid Taiko inbox address: {:?}", e))?;

        let taiko_geth_status = Arc::new(Mutex::new(TaikoGethStatus {
            height: 0,
            hash: B256::ZERO,
            expected_reorg: None,
        }));
        Ok(Self {
            ws_l1_rpc_url,
            ws_l2_rpc_url,
            taiko_inbox,
            taiko_geth_status,
            cancel_token,
        })
    }

    pub async fn set_expected_reorg(&self, expected_block_number: u64) {
        let mut status = self.taiko_geth_status.lock().await;
        status.expected_reorg = Some(expected_block_number);
    }

    /// Spawns the event listeners and the message handler.
    pub async fn start(&self) -> Result<(), Error> {
        debug!("Starting ChainMonitor");

        //BatchProposed events
        let (batch_proposed_tx, batch_proposed_rx) = mpsc::channel(MESSAGE_QUEUE_SIZE);
        let batch_receiver = BatchProposedEventReceiver::new(
            self.ws_l1_rpc_url.clone(),
            self.taiko_inbox,
            batch_proposed_tx,
            self.cancel_token.clone(),
        )
        .await?;
        batch_receiver.start();

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
            batch_proposed_rx,
            l2_block_rx,
            taiko_geth_status,
            cancel_token,
        ));

        Ok(())
    }

    async fn handle_incoming_messages(
        mut batch_proposed_rx: Receiver<ITaikoInbox::BatchProposed>,
        mut l2_block_rx: Receiver<L2BlockInfo>,
        taiko_geth_status: Arc<Mutex<TaikoGethStatus>>,
        cancel_token: CancellationToken,
    ) {
        info!("ChainMonitor message loop running");

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    info!("ChainMonitor: cancellation received, shutting down message loop");
                    break;
                }
                Some(batch) = batch_proposed_rx.recv() => {
                    info!(
                        "BatchProposed event → lastBlockId = {}",
                        batch.info.lastBlockId
                    );
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
                            if !reorg_expected {
                                tracing::warn!("⛔ Geth reorg detected: Received L2 block with unexpected number. Expected: block_id {} hash {}", status.height, status.hash);
                            } else {
                                tracing::debug!("Geth reorg detected: Received L2 block with expected number. Expected: block_id {} hash {}", status.height, status.hash);
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
