mod config;
pub mod models;
mod operation_type;
mod status_provider_trait;

use crate::{metrics::Metrics, utils::rpc_client::HttpRPCClient};
use anyhow::Error;
pub use config::TaikoDriverConfig;
use models::{
    BuildPreconfBlockRequestBody, BuildPreconfBlockResponse, ReorgStaleBlockRequest,
    ReorgStaleBlockResponse, TaikoStatus,
};
pub use operation_type::OperationType;
use serde_json::Value;
pub use status_provider_trait::StatusProvider;
use std::sync::Arc;
use std::time::Duration;

pub struct TaikoDriver {
    preconf_rpc: HttpRPCClient,
    status_rpc: HttpRPCClient,
    metrics: Arc<Metrics>,
    retry_timeout: Duration,
}

impl TaikoDriver {
    pub async fn new(config: &TaikoDriverConfig, metrics: Arc<Metrics>) -> Result<Self, Error> {
        Ok(Self {
            preconf_rpc: HttpRPCClient::new_with_jwt(
                &config.driver_url,
                config.rpc_driver_preconf_timeout,
                &config.jwt_secret_bytes,
            )
            .map_err(|e| {
                anyhow::anyhow!("Failed to create HttpRPCClient for driver preconf: {}", e)
            })?,
            status_rpc: HttpRPCClient::new_with_jwt(
                &config.driver_url,
                config.rpc_driver_status_timeout,
                &config.jwt_secret_bytes,
            )
            .map_err(|e| {
                anyhow::anyhow!("Failed to create HttpRPCClient for driver status: {}", e)
            })?,
            metrics,
            retry_timeout: config.rpc_driver_retry_timeout,
        })
    }

    pub async fn preconf_blocks(
        &self,
        request_body: BuildPreconfBlockRequestBody,
        operation_type: OperationType,
    ) -> Result<BuildPreconfBlockResponse, Error> {
        const API_ENDPOINT: &str = "preconfBlocks";

        let response = self
            .call_driver(
                &self.preconf_rpc,
                http::Method::POST,
                API_ENDPOINT,
                &request_body,
                operation_type,
            )
            .await?;

        if let Some(preconfirmed_block) =
            BuildPreconfBlockResponse::new_from_value(response, request_body.is_forced_inclusion)
        {
            self.metrics.inc_blocks_preconfirmed();
            Ok(preconfirmed_block)
        } else {
            Err(anyhow::anyhow!(
                "Block was preconfirmed, but failed to decode response from driver."
            ))
        }
    }

    pub async fn reorg_stale_block(
        &self,
        new_head_block_number: u64,
    ) -> Result<ReorgStaleBlockResponse, Error> {
        const API_ENDPOINT: &str = "reorgStaleBlock";

        let request_body = ReorgStaleBlockRequest {
            new_head_block_number,
        };

        let response = self
            .call_driver(
                &self.preconf_rpc,
                http::Method::POST,
                API_ENDPOINT,
                &request_body,
                OperationType::ReorgStaleBlock,
            )
            .await?;

        let reorg_response: ReorgStaleBlockResponse = serde_json::from_value(response)?;
        Ok(reorg_response)
    }

    async fn call_driver<T>(
        &self,
        client: &HttpRPCClient,
        method: http::Method,
        endpoint: &str,
        payload: &T,
        operation_type: OperationType,
    ) -> Result<Value, Error>
    where
        T: serde::Serialize,
    {
        let metric_label = operation_type.to_string();
        self.metrics.inc_rpc_driver_call(&metric_label);
        let start_time = std::time::Instant::now();

        match client
            .retry_request_with_timeout(method, endpoint, payload, self.retry_timeout)
            .await
        {
            Ok(response) => {
                self.metrics.observe_rpc_driver_call_duration(
                    &metric_label,
                    start_time.elapsed().as_secs_f64(),
                );
                Ok(response)
            }
            Err(e) => {
                self.metrics.inc_rpc_driver_call_error(&metric_label);
                let metric_label_error = format!("{metric_label}-error");
                self.metrics.observe_rpc_driver_call_duration(
                    &metric_label_error,
                    start_time.elapsed().as_secs_f64(),
                );
                Err(e)
            }
        }
    }
}

impl StatusProvider for TaikoDriver {
    async fn get_status(&self) -> Result<TaikoStatus, Error> {
        const API_ENDPOINT: &str = "status";
        let request_body = serde_json::json!({});

        let response = self
            .call_driver(
                &self.status_rpc,
                http::Method::GET,
                API_ENDPOINT,
                &request_body,
                OperationType::Status,
            )
            .await?;

        let status: TaikoStatus = serde_json::from_value(response)?;

        Ok(status)
    }
}
