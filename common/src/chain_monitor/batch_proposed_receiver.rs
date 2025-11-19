use crate::utils::{cancellation_token::CancellationToken, event_listener::listen_for_event};
use alloy::primitives::Address;
use alloy::sol_types::SolEvent;
use anyhow::Error;
use tokio::{sync::mpsc::Sender, time::Duration};
use tracing::info;

const SLEEP_DURATION: Duration = Duration::from_secs(15);

pub struct EventReceiver<T> {
    ws_rpc_url: String,
    contract_address: Address,
    event_tx: Sender<T>,
    cancel_token: CancellationToken,
    event_name: &'static str,
}

impl<T> EventReceiver<T>
where
    T: SolEvent + Send + 'static,
{
    pub async fn new(
        ws_rpc_url: String,
        contract_address: Address,
        event_tx: Sender<T>,
        cancel_token: CancellationToken,
        event_name: &'static str,
    ) -> Result<Self, Error> {
        Ok(Self {
            ws_rpc_url,
            contract_address,
            event_tx,
            cancel_token,
            event_name,
        })
    }

    pub fn start(&self) {
        info!("Starting {} event receiver", self.event_name);
        let ws_rpc_url = self.ws_rpc_url.clone();
        let contract_address = self.contract_address;
        let event_tx = self.event_tx.clone();
        let cancel_token = self.cancel_token.clone();
        let event_name = self.event_name;

        tokio::spawn(async move {
            listen_for_event(
                ws_rpc_url,
                contract_address,
                event_name,
                T::SIGNATURE_HASH,
                |log| Ok(T::decode_log(&log.inner)?.data),
                event_tx,
                cancel_token,
                SLEEP_DURATION,
            )
            .await;
        });
    }
}
