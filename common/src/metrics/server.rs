use crate::metrics::Metrics;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::info;
use warp::Filter;

pub fn serve_metrics(metrics: Arc<Metrics>, cancel_token: CancellationToken) {
    tokio::spawn(async move {
        let route = warp::path!("metrics").map(move || {
            let output = metrics.gather();
            warp::reply::with_header(output, "Content-Type", "text/plain; version=0.0.4")
        });

        let (addr, server) =
            warp::serve(route).bind_with_graceful_shutdown(([0, 0, 0, 0], 9898), async move {
                cancel_token.cancelled().await;
                info!("Shutdown signal received, stopping metrics server...");
            });

        info!("Metrics server listening on {}", addr);
        server.await;
    });
}
