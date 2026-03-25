use crate::utils::{
    cancellation_token::CancellationToken,
    event_listener::{EventListenerConfig, listen_for_event},
};
use alloy::primitives::Address;
use alloy::sol_types::SolEvent;
use anyhow::Error;
use tokio::{sync::mpsc::Sender, time::Duration};
use tracing::info;

const RECONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_secs(12);

pub struct EventReceiver<T> {
    rpc_url: String,
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
        rpc_url: String,
        contract_address: Address,
        event_tx: Sender<T>,
        cancel_token: CancellationToken,
        event_name: &'static str,
    ) -> Result<Self, Error> {
        Ok(Self {
            rpc_url,
            contract_address,
            event_tx,
            cancel_token,
            event_name,
        })
    }

    pub fn start(&self) {
        info!("Starting {} event receiver", self.event_name);
        let rpc_url = self.rpc_url.clone();
        let contract_address = self.contract_address;
        let event_tx = self.event_tx.clone();
        let cancel_token = self.cancel_token.clone();
        let event_name = self.event_name;

        tokio::spawn(async move {
            listen_for_event(
                EventListenerConfig {
                    rpc_url,
                    contract_address,
                    event_name,
                    signature_hash: T::SIGNATURE_HASH,
                    reconnect_timeout: RECONNECT_TIMEOUT,
                    poll_interval: POLL_INTERVAL,
                },
                |log| Ok(T::decode_log(&log.inner)?.data),
                event_tx,
                cancel_token,
            )
            .await;
        });
    }
}
