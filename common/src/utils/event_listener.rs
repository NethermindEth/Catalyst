use crate::shared::alloy_tools;
use crate::utils::cancellation_token::CancellationToken;
use alloy::{
    primitives::{Address, B256},
    providers::{DynProvider, Provider},
    rpc::types::{Filter, Log},
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

const MAX_BLOCKS_PER_POLL: u64 = 10;

pub struct EventListenerConfig {
    pub rpc_url: String,
    pub contract_address: Address,
    pub event_name: &'static str,
    pub signature_hash: B256,
    pub reconnect_timeout: Duration,
    pub poll_interval: Duration,
}

pub async fn listen_for_event<T>(
    config: EventListenerConfig,
    to_event: fn(Log) -> Result<T, Error>,
    sender_tx: Sender<T>,
    cancel_token: CancellationToken,
) where
    T: Send + SolEvent,
{
    let EventListenerConfig {
        rpc_url,
        contract_address,
        event_name,
        signature_hash,
        reconnect_timeout,
        poll_interval,
    } = config;

    loop {
        if cancel_token.is_cancelled() {
            info!("{event_name}: cancellation requested, exiting");
            return;
        }

        let provider = match alloy_tools::create_alloy_provider_without_wallet(&rpc_url).await {
            Ok(p) => p,
            Err(e) => {
                error!("{event_name}: failed to create provider: {e:?}");
                sleep(reconnect_timeout).await;
                continue;
            }
        };

        let filter = Filter::new()
            .address(contract_address)
            .event_signature(signature_hash);

        let reconnect = match provider.subscribe_logs(&filter).await {
            Ok(subscription) => {
                info!("{event_name}: subscribed via WebSocket");
                let mut stream = subscription.into_stream();
                run_subscription_loop(&mut stream, event_name, to_event, &sender_tx, &cancel_token)
                    .await
            }
            Err(e) => {
                info!("{event_name}: subscription failed ({e:?}), falling back to HTTP polling");
                run_polling_loop(
                    &provider,
                    filter,
                    event_name,
                    to_event,
                    &sender_tx,
                    &cancel_token,
                    poll_interval,
                )
                .await
            }
        };

        if reconnect {
            warn!("{event_name}: stream ended or errored; reconnecting in {reconnect_timeout:?}");
            sleep(reconnect_timeout).await;
        } else {
            return;
        }
    }
}

/// Drives the WebSocket subscription stream until it ends or is cancelled.
/// Returns `true` to trigger a reconnect, `false` on cancellation.
async fn run_subscription_loop<T>(
    stream: &mut (impl StreamExt<Item = Log> + Unpin),
    event_name: &'static str,
    to_event: fn(Log) -> Result<T, Error>,
    sender_tx: &Sender<T>,
    cancel_token: &CancellationToken,
) -> bool
where
    T: Send + SolEvent,
{
    loop {
        select! {
            _ = cancel_token.cancelled() => {
                info!("{event_name}: cancellation received");
                return false;
            }
            result = stream.next() => match result {
                Some(log) => {
                    if dispatch_log(log, event_name, to_event, sender_tx).await.is_err() {
                        return true;
                    }
                }
                None => {
                    warn!("{event_name}: event stream ended unexpectedly");
                    return true;
                }
            }
        }
    }
}

/// Polls for new logs over HTTP until an error occurs or is cancelled.
/// Returns `true` to trigger a reconnect, `false` on cancellation.
async fn run_polling_loop<T>(
    provider: &DynProvider,
    filter: Filter,
    event_name: &'static str,
    to_event: fn(Log) -> Result<T, Error>,
    sender_tx: &Sender<T>,
    cancel_token: &CancellationToken,
    poll_interval: Duration,
) -> bool
where
    T: Send + SolEvent,
{
    let mut next_block = match provider.get_block_number().await {
        Ok(n) => n.saturating_add(1),
        Err(e) => {
            error!("{event_name}: failed to initialise polling block height: {e:?}");
            return true;
        }
    };
    debug!("{event_name}: polling from block {next_block}");

    loop {
        select! {
            _ = cancel_token.cancelled() => {
                info!("{event_name}: cancellation received, stopping polling");
                return false;
            }
            _ = sleep(poll_interval) => {}
        }

        let latest_block = match provider.get_block_number().await {
            Ok(n) => n,
            Err(e) => {
                error!("{event_name}: failed to fetch latest block: {e:?}");
                return true;
            }
        };

        if latest_block < next_block {
            continue;
        }

        if latest_block - next_block > MAX_BLOCKS_PER_POLL {
            debug!(
                "{event_name}: gap too large: latest={latest_block}, next={next_block}, fetching last {MAX_BLOCKS_PER_POLL}"
            );
            next_block = latest_block - MAX_BLOCKS_PER_POLL;
        }

        let logs = match provider
            .get_logs(&filter.clone().from_block(next_block).to_block(latest_block))
            .await
        {
            Ok(logs) => logs,
            Err(e) => {
                error!("{event_name}: failed to fetch logs: {e:?}");
                return true;
            }
        };

        for log in logs {
            if dispatch_log(log, event_name, to_event, sender_tx)
                .await
                .is_err()
            {
                return true;
            }
        }

        next_block = latest_block.saturating_add(1);
    }
}

async fn dispatch_log<T>(
    log: Log,
    event_name: &'static str,
    to_event: fn(Log) -> Result<T, Error>,
    sender_tx: &Sender<T>,
) -> Result<(), ()>
where
    T: Send + SolEvent,
{
    match to_event(log) {
        Ok(event) => sender_tx.send(event).await.map_err(|e| {
            error!("{event_name}: failed to send event: {e:?}");
        }),
        Err(e) => {
            error!("{event_name}: failed to decode event: {e:?}");
            Err(())
        }
    }
}
