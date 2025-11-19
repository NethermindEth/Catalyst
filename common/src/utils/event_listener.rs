use crate::utils::cancellation_token::CancellationToken;
use alloy::{
    primitives::{Address, B256},
    providers::{Provider, ProviderBuilder, WsConnect},
    rpc::types::Filter,
    sol_types::SolEvent,
};
use anyhow::Error;
use futures_util::StreamExt;
use tokio::{
    select,
    sync::mpsc::Sender,
    time::{Duration, sleep},
};
use tracing::{debug, error, info, warn};

#[allow(clippy::too_many_arguments)]
pub async fn listen_for_event<T>(
    ws_rpc_url: String,
    contract_address: Address,
    event_name: &'static str,
    signature_hash: B256,
    to_event: fn(alloy::rpc::types::Log) -> Result<T, Error>,
    sender_tx: Sender<T>,
    cancel_token: CancellationToken,
    reconnect_timeout: Duration,
) where
    T: Send + SolEvent,
{
    loop {
        if cancel_token.is_cancelled() {
            info!("{event_name}Receiver: cancellation requested, exiting loop");
            break;
        }

        let ws = WsConnect::new(ws_rpc_url.clone());

        let provider_ws = match ProviderBuilder::new().connect_ws(ws).await {
            Ok(provider) => provider,
            Err(e) => {
                error!(
                    "Failed to create WebSocket provider for {event_name}: {:?}",
                    e
                );
                sleep(reconnect_timeout).await;
                continue;
            }
        };

        let filter = Filter::new()
            .address(contract_address)
            .event_signature(signature_hash);

        let mut stream = match provider_ws.subscribe_logs(&filter).await {
            Ok(subscription) => {
                debug!("Subscribed to {event_name} events");
                subscription.into_stream()
            }
            Err(e) => {
                error!("Failed to subscribe to {event_name}: {:?}", e);
                sleep(reconnect_timeout).await;
                continue;
            }
        };

        loop {
            select! {
                _ = cancel_token.cancelled() => {
                    info!("{event_name}: cancellation received, stopping event loop");
                    return;
                }
                result = stream.next() => {
                    match result {
                        Some(log) => {
                            match to_event(log) {
                                Ok(event) => {
                                    if let Err(e) = sender_tx.send(event).await {
                                        error!("Failed to send {event_name} event: {:?}", e);
                                        break;
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to decode {event_name} event: {:?}", e);
                                    break;
                                }
                            }
                        }
                        None => {
                            warn!("{event_name} event stream ended unexpectedly");
                            break;
                        }
                    }
                }
            }
        }

        if !cancel_token.is_cancelled() {
            warn!(
                "{event_name} event stream ended or errored; reconnecting in {:?}",
                reconnect_timeout
            );
            sleep(reconnect_timeout).await;
        }
    }
}
