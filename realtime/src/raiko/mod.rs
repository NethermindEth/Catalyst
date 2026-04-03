use crate::l1::bindings::ProofType;
use crate::utils::config::RealtimeConfig;
use anyhow::Error;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{debug, info, warn};

#[derive(Clone)]
pub struct RaikoClient {
    client: Client,
    pub base_url: String,
    pub api_key: Option<String>,
    pub proof_type: ProofType,
    #[allow(dead_code)]
    l2_network: String,
    #[allow(dead_code)]
    l1_network: String,
    poll_interval: Duration,
    max_retries: u32,
}

#[derive(Serialize)]
pub struct RaikoProofRequest {
    pub l2_block_numbers: Vec<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub l2_block_hashes: Option<Vec<String>>,
    pub proof_type: String,
    pub max_anchor_block_number: u64,
    pub last_finalized_block_hash: String,
    pub basefee_sharing_pctg: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub l1_network: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prover: Option<String>,
    pub signal_slots: Vec<String>,
    pub sources: Vec<RaikoDerivationSource>,
    pub blobs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<RaikoCheckpoint>,
    pub blob_proof_type: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RaikoDerivationSource {
    pub is_forced_inclusion: bool,
    pub blob_slice: RaikoBlobSlice,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RaikoBlobSlice {
    pub blob_hashes: Vec<String>,
    pub offset: u32,
    pub timestamp: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct RaikoCheckpoint {
    pub block_number: u64,
    pub block_hash: String,
    pub state_root: String,
}

#[derive(Deserialize)]
pub struct RaikoResponse {
    pub status: String,
    #[serde(default)]
    pub data: Option<RaikoData>,
    #[serde(default)]
    pub proof_type: Option<String>,
    #[serde(default)]
    pub batch_id: Option<u64>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum RaikoData {
    Proof { proof: RaikoProof },
    Status { status: String },
}

#[derive(Deserialize)]
pub struct RaikoProof {
    pub proof: Option<String>,
    #[serde(default)]
    pub input: Option<String>,
    #[serde(default)]
    pub quote: Option<String>,
    #[serde(default)]
    pub uuid: Option<String>,
    #[serde(default)]
    pub kzg_proof: Option<String>,
}

impl RaikoClient {
    pub fn new(config: &RealtimeConfig) -> Self {
        Self {
            client: Client::new(),
            base_url: config.raiko_url.clone(),
            api_key: config.raiko_api_key.clone(),
            proof_type: config.proof_type,
            l2_network: config.raiko_network.clone(),
            l1_network: config.raiko_l1_network.clone(),
            poll_interval: Duration::from_millis(config.raiko_poll_interval_ms),
            max_retries: config.raiko_max_retries,
        }
    }

    /// Submit a proof request and poll until ready.
    ///
    /// Flow:
    /// 1. First request sends full sources+blobs to trigger proving.
    /// 2. Subsequent polls send empty sources+blobs (the cache key fields —
    ///    `l2_block_numbers`, `l2_block_hashes`, etc. — are still included).
    /// 3. If a poll returns "proof not found" (expired from cache), we
    ///    re-submit the full request with sources+blobs.
    pub async fn get_proof(&self, request: &RaikoProofRequest) -> Result<Vec<u8>, Error> {
        let url = format!("{}/v3/proof/batch/realtime", self.base_url);

        // Build the polling variant: same cache-key fields, empty sources+blobs.
        let poll_request = RaikoProofRequest {
            l2_block_numbers: request.l2_block_numbers.clone(),
            l2_block_hashes: request.l2_block_hashes.clone(),
            proof_type: request.proof_type.clone(),
            max_anchor_block_number: request.max_anchor_block_number,
            last_finalized_block_hash: request.last_finalized_block_hash.clone(),
            basefee_sharing_pctg: request.basefee_sharing_pctg,
            network: request.network.clone(),
            l1_network: request.l1_network.clone(),
            prover: request.prover.clone(),
            signal_slots: request.signal_slots.clone(),
            sources: vec![],
            blobs: vec![],
            checkpoint: request.checkpoint.clone(),
            blob_proof_type: request.blob_proof_type.clone(),
        };

        // First attempt always sends the full request to trigger proving.
        let mut use_full_request = true;

        for attempt in 0..self.max_retries {
            let body_to_send = if use_full_request {
                request
            } else {
                &poll_request
            };

            let mut req = self.client.post(&url).json(body_to_send);
            if let Some(ref key) = self.api_key {
                req = req.header("X-API-KEY", key);
            }

            let resp = req.send().await?;
            let http_status = resp.status();
            let raw_body = resp.text().await?;
            debug!(
                "Raiko response (attempt {}, full={}): HTTP {} | body: {}",
                attempt + 1,
                use_full_request,
                http_status,
                raw_body
            );

            // After the first submission, switch to polling (empty sources+blobs).
            use_full_request = false;

            let body: RaikoResponse = serde_json::from_str(&raw_body).map_err(|e| {
                anyhow::anyhow!(
                    "Failed to parse Raiko response (HTTP {}): {} | body: {}",
                    http_status,
                    e,
                    raw_body
                )
            })?;

            if body.status == "error" {
                let msg = body.message.unwrap_or_default();
                // "proof not found" means the proof expired from cache (Redis TTL) or was
                // never submitted. Re-submit the full request with sources+blobs.
                if msg.contains("proof not found") {
                    warn!(
                        "Raiko: proof not found (expired or never submitted), re-submitting... (attempt {})",
                        attempt + 1
                    );
                    use_full_request = true;
                    tokio::time::sleep(self.poll_interval).await;
                    continue;
                }
                return Err(anyhow::anyhow!("Raiko proof failed: {}", msg));
            }

            match body.data {
                Some(RaikoData::Proof { proof: proof_obj }) => {
                    let proof_hex = proof_obj.proof.ok_or_else(|| {
                        anyhow::anyhow!("Raiko returned proof object with null proof field")
                    })?;
                    info!("ZK proof received (attempt {})", attempt + 1);
                    let proof_bytes = hex::decode(proof_hex.trim_start_matches("0x"))?;
                    return Ok(proof_bytes);
                }
                Some(RaikoData::Status { ref status }) if status == "ZKAnyNotDrawn" => {
                    warn!("Raiko: ZK prover not drawn for this request");
                    return Err(anyhow::anyhow!("ZK prover not drawn"));
                }
                Some(RaikoData::Status { ref status }) => {
                    debug!(
                        "Raiko status: {}, polling... (attempt {})",
                        status,
                        attempt + 1
                    );
                    tokio::time::sleep(self.poll_interval).await;
                }
                None => {
                    return Err(anyhow::anyhow!("Raiko: unexpected empty response"));
                }
            }
        }

        Err(anyhow::anyhow!(
            "Raiko: proof not ready after {} attempts",
            self.max_retries
        ))
    }
}
