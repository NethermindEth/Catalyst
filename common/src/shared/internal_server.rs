use crate::utils::cancellation_token::CancellationToken;
use axum::Router;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tracing::error;
use tracing::info;

pub fn serve_metrics(cancel_token: CancellationToken, routes: Vec<Router>) {
    tokio::spawn(async move {
        let app = routes
            .into_iter()
            .fold(Router::new(), |app, router| app.merge(router));

        let addr: SocketAddr = ([0, 0, 0, 0], 9898).into();
        info!("Internal server listening on {}", addr);
        let listener = match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => listener,
            Err(err) => {
                error!(
                    "Failed to bind internal server listener on {}: {}",
                    addr, err
                );
                return;
            }
        };

        serve(listener, app, cancel_token).await;
    });
}

async fn serve(listener: TcpListener, app: Router, shutdown_token: CancellationToken) {
    if let Err(err) = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_token.cancelled().await;
            info!("Shutdown signal received, stopping internal server...");
        })
        .await
    {
        error!("Internal server terminated with error: {}", err);
    }
}
