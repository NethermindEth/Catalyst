use crate::l1::bindings::taiko_inbox::ITaikoInbox;
use alloy::primitives::Address;
use alloy::sol_types::SolEvent;
use anyhow::Error;
use common::utils::event_listener::listen_for_event;
use tokio::{sync::mpsc::Sender, time::Duration};
use tokio_util::sync::CancellationToken;
use tracing::info;

const SLEEP_DURATION: Duration = Duration::from_secs(15);

pub struct BatchProposedEventReceiver {
    ws_rpc_url: String,
    taiko_inbox: Address,
    batch_proposed_tx: Sender<ITaikoInbox::BatchProposed>,
    cancel_token: CancellationToken,
}

impl BatchProposedEventReceiver {
    pub async fn new(
        ws_rpc_url: String,
        taiko_inbox: Address,
        batch_proposed_tx: Sender<ITaikoInbox::BatchProposed>,
        cancel_token: CancellationToken,
    ) -> Result<Self, Error> {
        Ok(Self {
            ws_rpc_url,
            taiko_inbox,
            batch_proposed_tx,
            cancel_token,
        })
    }

    pub fn start(&self) {
        info!("Starting BatchProposed event receiver");
        let ws_rpc_url = self.ws_rpc_url.clone();
        let taiko_inbox = self.taiko_inbox;
        let batch_proposed_tx = self.batch_proposed_tx.clone();
        let cancel_token = self.cancel_token.clone();

        tokio::spawn(async move {
            listen_for_event(
                ws_rpc_url,
                taiko_inbox,
                "BatchProposed",
                ITaikoInbox::BatchProposed::SIGNATURE_HASH,
                |log| Ok(ITaikoInbox::BatchProposed::decode_log(&log.inner)?.data),
                batch_proposed_tx,
                cancel_token,
                SLEEP_DURATION,
            )
            .await;
        });
    }
}
