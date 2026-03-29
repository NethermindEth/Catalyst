use crate::metrics::Metrics;
use crate::utils::cancellation_token::CancellationToken;
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::info;
use warp::Filter;

pub fn serve_metrics(metrics: Arc<Metrics>, cancel_token: CancellationToken) {
    tokio::spawn(async move {
        let route = warp::path!("metrics").map(move || {
            let output = metrics.gather();
            warp::reply::with_header(output, "Content-Type", "text/plain; version=0.0.4")
        });

        let addr: SocketAddr = ([0, 0, 0, 0], 9898).into();
        info!("Metrics server listening on {}", addr);
        let server = warp::serve(route).bind(addr).await;

        let shutdown_token = cancel_token.clone();
        server
            .graceful(async move {
                shutdown_token.cancelled().await;
                info!("Shutdown signal received, stopping metrics server...");
            })
            .run()
            .await;
    });
}
