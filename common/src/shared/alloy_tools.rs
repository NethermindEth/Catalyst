use crate::signer::Signer;
use alloy::{
    network::{Ethereum, EthereumWallet},
    primitives::B256,
    providers::{DynProvider, Provider, ProviderBuilder, WsConnect, ext::DebugApi},
    pubsub::{PubSubConnect, PubSubFrontend},
    rpc::client::RpcClient,
    rpc::types::{Transaction, TransactionRequest, trace::geth::GethDebugTracingOptions},
    signers::local::PrivateKeySigner,
    transports::{
        http::{Http, reqwest::Url},
        layers::FallbackLayer,
    },
};
use anyhow::Error;
use futures_util::future;
use std::{num::NonZeroUsize, str::FromStr};
use tower::ServiceBuilder;
use tracing::{debug, warn};

pub async fn check_for_revert_reason<P: Provider<Ethereum>>(
    provider: &P,
    tx_hash: B256,
    block_number: u64,
) -> String {
    let default_options = GethDebugTracingOptions::default();
    let trace = provider
        .debug_trace_transaction(tx_hash, default_options)
        .await;

    let trace_errors = if let Ok(trace) = trace {
        find_errors_from_trace(&format!("{trace:?}"))
    } else {
        None
    };

    let tx_details = match provider.get_transaction_by_hash(tx_hash).await {
        Ok(Some(tx)) => tx,
        _ => {
            return format!("Transaction {tx_hash} failed");
        }
    };

    let call_request = get_tx_request_for_call(tx_details);
    let revert_reason = match provider.call(call_request).block(block_number.into()).await {
        Err(e) => e.to_string(),
        Ok(ok) => format!("Unknown revert reason: {ok}"),
    };

    let mut error_msg = format!("Transaction {tx_hash} failed: {revert_reason}");
    if let Some(trace_errors) = trace_errors {
        error_msg.push_str(&trace_errors);
    }
    error_msg
}

fn get_tx_request_for_call(tx_details: Transaction) -> TransactionRequest {
    TransactionRequest::from_transaction(tx_details)
}

fn find_errors_from_trace(trace_str: &str) -> Option<String> {
    let mut start_pos = 0;
    let mut error_message = String::new();
    while let Some(error_start) = trace_str[start_pos..].find("error: Some(") {
        let absolute_pos = start_pos + error_start;
        if let Some(closing_paren) = trace_str[absolute_pos..].find(')') {
            let error_content = &trace_str[absolute_pos..absolute_pos + closing_paren + 1];
            if !error_message.is_empty() {
                error_message.push_str(", ");
            }
            error_message.push_str(error_content);
            start_pos = absolute_pos + closing_paren + 1;
        } else {
            break;
        }
    }
    if !error_message.is_empty() {
        Some(format!(", errors from debug trace: {error_message}"))
    } else {
        None
    }
}

pub async fn construct_alloy_provider(
    signer: &Signer,
    execution_ws_rpc_urls: &[String],
) -> Result<DynProvider, Error> {
    match signer {
        Signer::PrivateKey(private_key, _) => {
            debug!(
                "Creating alloy provider with URLs: {:?} and private key signer.",
                execution_ws_rpc_urls
            );
            let signer = PrivateKeySigner::from_str(private_key.as_str())?;

            Ok(create_alloy_provider_with_wallet(signer.into(), execution_ws_rpc_urls).await?)
        }
        Signer::Web3signer(web3signer, address) => {
            debug!(
                "Creating alloy provider with URLs: {:?} and web3signer signer.",
                execution_ws_rpc_urls
            );
            let preconfer_address = *address;

            let tx_signer = crate::signer::web3signer::Web3TxSigner::new(
                web3signer.clone(),
                preconfer_address,
            )?;
            let wallet = EthereumWallet::new(tx_signer);

            Ok(create_alloy_provider_with_wallet(wallet, execution_ws_rpc_urls).await?)
        }
    }
}

async fn create_alloy_provider_with_wallet(
    wallet: EthereumWallet,
    urls: &[String],
) -> Result<DynProvider, Error> {
    let client = if urls
        .iter()
        .all(|url| url.starts_with("ws://") || url.starts_with("wss://"))
    {
        let transports = create_websocket_transports(urls).await?;

        let fallback_layer = FallbackLayer::default().with_active_transport_count(
            NonZeroUsize::new(transports.len()).ok_or_else(|| {
                anyhow::anyhow!("Failed to create NonZeroUsize from transports.len()")
            })?,
        );
        RpcClient::builder().transport(
            ServiceBuilder::new()
                .layer(fallback_layer)
                .service(transports),
            false,
        )
    } else if urls
        .iter()
        .all(|url| url.contains("http://") || url.contains("https://"))
    {
        let transports = create_http_transports(urls)?;

        let fallback_layer = FallbackLayer::default().with_active_transport_count(
            NonZeroUsize::new(transports.len()).ok_or_else(|| {
                anyhow::anyhow!("Failed to create NonZeroUsize from transports.len()")
            })?,
        );
        RpcClient::builder().transport(
            ServiceBuilder::new()
                .layer(fallback_layer)
                .service(transports),
            false,
        )
    } else {
        return Err(anyhow::anyhow!(
            "Invalid URL list, only websocket and http are supported, you cannot mix websockets and HTTP URLs: {}",
            urls.join(", ")
        ));
    };

    Ok(ProviderBuilder::new()
        .wallet(wallet)
        .connect_client(client)
        .erased())
}

async fn create_websocket_transports(urls: &[String]) -> Result<Vec<PubSubFrontend>, Error> {
    let connection_futures = urls.iter().map(|url| async move {
        WsConnect::new(url)
            .into_service()
            .await
            .map(|ws| (url, ws))
            .inspect(|_| debug!("Connected to {url}"))
            .inspect_err(|e| warn!("Failed to connect to {url}: {e}"))
    });

    let transports: Vec<_> = future::join_all(connection_futures)
        .await
        .into_iter()
        .filter_map(Result::ok)
        .map(|(_, ws)| ws)
        .collect();

    if transports.is_empty() {
        return Err(anyhow::anyhow!(
            "No valid WebSocket connections established"
        ));
    }

    Ok(transports)
}

fn create_http_transports(urls: &[String]) -> Result<Vec<Http<reqwest::Client>>, Error> {
    urls.iter()
        .map(|url| {
            Url::parse(url)
                .map_err(|e| {
                    anyhow::anyhow!(
                        "Failed to parse URL while creating HTTP transport for alloy provider: {e}"
                    )
                })
                .map(Http::new)
        })
        .collect()
}

pub async fn create_alloy_provider_without_wallet(url: &str) -> Result<DynProvider, Error> {
    if url.contains("ws://") || url.contains("wss://") {
        let ws = WsConnect::new(url);
        Ok(ProviderBuilder::new()
            .connect_ws(ws.clone())
            .await
            .map_err(|e| Error::msg(format!("Execution layer: Failed to connect to WS: {e}")))?
            .erased())
    } else if url.contains("http://") || url.contains("https://") {
        Ok(ProviderBuilder::new()
            .connect_http(url.parse::<reqwest::Url>()?)
            .erased())
    } else {
        Err(anyhow::anyhow!(
            "Invalid URL, only websocket and http are supported: {}",
            url
        ))
    }
}
