use crate::utils::cancellation_token::CancellationToken;
use axum::Router;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tracing::{error, info};

/// Spawns an internal HTTP server that merges the provided routes and listens on the given IP and
/// port. The server shuts down gracefully when the `cancel_token` is cancelled.
///
/// Known routes (registered by callers):
/// - `GET /metrics` — Prometheus metrics (all protocol variants)
/// - `GET /status`  — Node status (Shasta only)
pub fn serve(cancel_token: CancellationToken, routes: Vec<Router>, ip: [u8; 4], port: u16) {
    let addr = SocketAddr::from((ip, port));
    tokio::spawn(async move {
        let app = build_app(routes);

        info!("Internal server listening on {}", addr);

        let listener = match TcpListener::bind(addr).await {
            Ok(listener) => listener,
            Err(err) => {
                error!(
                    "Failed to bind internal server listener on {}: {}",
                    addr, err
                );
                return;
            }
        };

        run_server(listener, app, cancel_token).await;
    });
}

fn build_app(routes: Vec<Router>) -> Router {
    routes
        .into_iter()
        .fold(Router::new(), |app, router| app.merge(router))
}

async fn run_server(listener: TcpListener, app: Router, shutdown_token: CancellationToken) {
    let shutdown = async move {
        shutdown_token.cancelled().await;
        info!("Shutdown signal received, stopping internal server...");
    };

    if let Err(err) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
    {
        error!("Internal server terminated with error: {}", err);
    }
}
