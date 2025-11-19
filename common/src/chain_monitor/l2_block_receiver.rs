use crate::utils::cancellation_token::CancellationToken;
use alloy::primitives::B256;
use alloy::providers::{Provider, ProviderBuilder, WsConnect};
use anyhow::Error;
use tokio::{
    select,
    sync::mpsc::Sender,
    time::{Duration, sleep},
};
use tracing::{error, info, trace, warn};

const SLEEP_DURATION: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct L2BlockInfo {
    pub block_number: u64,
    pub block_hash: B256,
    pub parent_hash: B256,
}

pub struct L2BlockReceiver {
    ws_rpc_url: String,
    l2_block_info_tx: Sender<L2BlockInfo>,
    cancel_token: CancellationToken,
}

impl L2BlockReceiver {
    pub fn new(
        ws_rpc_url: String,
        l2_block_info_tx: Sender<L2BlockInfo>,
        cancel_token: CancellationToken,
    ) -> Self {
        Self {
            ws_rpc_url,
            l2_block_info_tx,
            cancel_token,
        }
    }

    pub fn start(&self) -> Result<(), Error> {
        let rpc_url = self.ws_rpc_url.clone();
        let l2_block_info_tx = self.l2_block_info_tx.clone();
        let cancel_token = self.cancel_token.clone();

        tokio::spawn(async move {
            loop {
                if cancel_token.is_cancelled() {
                    info!("L2BlockReceiver: cancellation requested, exiting loop");
                    break;
                }

                if let Err(e) = Self::listen_for_blocks(
                    &rpc_url,
                    l2_block_info_tx.clone(),
                    cancel_token.clone(),
                )
                .await
                {
                    error!("Error in block listener: {:?}", e);
                    sleep(SLEEP_DURATION).await;
                }
            }
        });

        Ok(())
    }

    async fn listen_for_blocks(
        rpc_url: &str,
        l2_block_info_tx: Sender<L2BlockInfo>,
        cancel_token: CancellationToken,
    ) -> Result<(), Error> {
        let ws = WsConnect::new(rpc_url.to_string());

        let provider_ws = ProviderBuilder::new().connect_ws(ws).await.map_err(|e| {
            error!("Failed to create WebSocket provider: {:?}", e);
            e
        })?;

        let mut subscription = provider_ws.subscribe_blocks().await.map_err(|e| {
            error!("Failed to subscribe to new blocks: {:?}", e);
            e
        })?;

        info!("Subscribed to L2 block headers");

        loop {
            select! {
                _ = cancel_token.cancelled() => {
                    info!("L2BlockReceiver: cancellation received, stopping block subscription loop");
                    break;
                }

                result = subscription.recv() => {
                    match result {
                        Ok(header) => {
                            let block_info = L2BlockInfo {
                                block_number: header.number,
                                block_hash: header.hash,
                                parent_hash: header.parent_hash,
                            };

                            trace!(
                                "Received Taiko block number: {}, hash: {}",
                                block_info.block_number, block_info.block_hash
                            );

                            if let Err(e) = l2_block_info_tx.send(block_info).await {
                                return Err(anyhow::anyhow!("Failed to send block info: {:?}", e));
                            }
                        }
                        Err(e) => {
                            warn!("Subscription error: {:?}", e);
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
