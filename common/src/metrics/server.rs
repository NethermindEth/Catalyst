use crate::metrics::Metrics;
use crate::utils::cancellation_token::CancellationToken;
use axum::Router;
use axum::extract::State;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::error;
use tracing::info;

async fn metrics_handler(State(metrics): State<Arc<Metrics>>) -> impl IntoResponse {
    let output = metrics.gather();
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        output,
    )
}

pub fn serve_metrics(metrics: Arc<Metrics>, cancel_token: CancellationToken) {
    tokio::spawn(async move {
        let app = Router::new()
            .route("/metrics", get(metrics_handler))
            .with_state(metrics);

        let addr: SocketAddr = ([0, 0, 0, 0], 9898).into();
        info!("Metrics server listening on {}", addr);
        let listener = match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => listener,
            Err(err) => {
                error!("Failed to bind metrics listener on {}: {}", addr, err);
                return;
            }
        };

        let shutdown_token = cancel_token.clone();
        if let Err(err) = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                shutdown_token.cancelled().await;
                info!("Shutdown signal received, stopping metrics server...");
            })
            .await
        {
            error!("Metrics server terminated with error: {}", err);
        }
    });
}
