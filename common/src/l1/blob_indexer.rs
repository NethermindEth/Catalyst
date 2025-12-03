use std::{error::Error as std_error, time::Duration};

use alloy::{consensus::Blob, primitives::B256};
use anyhow::Error;
use reqwest;

pub struct BlobIndexer {
    client: reqwest::Client,
    url: reqwest::Url,
}

impl BlobIndexer {
    pub fn new(rpc_url: &str, timeout: Duration) -> Result<Self, Error> {
        let client = reqwest::Client::builder().timeout(timeout).build()?;
        Ok(Self {
            client,
            url: reqwest::Url::parse(rpc_url)?,
        })
    }

    pub async fn get_blob(&self, hash: B256) -> Result<Blob, Error> {
        let response = self.get(&format!("v1/blobs/{hash}")).await?;

        self.parse_blob_from_response(response)
    }

    fn parse_blob_from_response(&self, response: serde_json::Value) -> Result<Blob, Error> {
        let data_hex = response
            .get("data")
            .and_then(|data| data.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing or invalid 'data' field in blob response"))?;

        let data_bytes = self.decode_hex_data(data_hex)?;

        Blob::try_from(data_bytes.as_slice())
            .map_err(|e| anyhow::anyhow!("Failed to deserialize blob data: {}", e))
    }

    fn decode_hex_data(&self, hex_str: &str) -> Result<Vec<u8>, Error> {
        let trimmed_hex = hex_str.trim_start_matches("0x");
        hex::decode(trimmed_hex)
            .map_err(|e| anyhow::anyhow!("Failed to decode hex data '{}': {}", hex_str, e))
    }

    async fn get(&self, path: &str) -> Result<serde_json::Value, Error> {
        let response = self
            .client
            .get(self.url.join(path)?)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    anyhow::anyhow!("Blob Indexer request timed out: {}", path)
                } else {
                    anyhow::anyhow!(
                        "Blob Indexer request failed with error: {}. Source: {:?}",
                        e,
                        e.source()
                    )
                }
            })?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "Blob Indexer request ({}) failed with status: {}",
                path,
                response.status()
            ));
        }

        let body = response.text().await?;
        let v: serde_json::Value = serde_json::from_str(&body)?;
        Ok(v)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use hex::FromHex;
    use tokio;

    #[tokio::test]
    async fn test_get_blob() {
        let hash = B256::from(
            <[u8; 32]>::from_hex(
                "018263025dafb83e4d0ae0ae8ce123ac4d32ca515901e74cc3c6dd9abb676aa6",
            )
            .unwrap(),
        );

        let server = setup_server().await;
        let cl = BlobIndexer::new(server.url().as_str(), Duration::from_secs(1)).unwrap();

        let blob = cl.get_blob(hash).await.unwrap();
        let sidecar =
            alloy::consensus::BlobTransactionSidecar::try_from_blobs_bytes(vec![blob]).unwrap();
        let versioned_hash = sidecar.versioned_hash_for_blob(0).unwrap();

        assert_eq!(
            versioned_hash, hash,
            "The versioned hash derived from the blob does not match the expected hash"
        );
    }

    async fn setup_server() -> mockito::ServerGuard {
        let mut server = mockito::Server::new_async().await;
        server
            .mock(
                "GET",
                "/v1/blobs/0x018263025dafb83e4d0ae0ae8ce123ac4d32ca515901e74cc3c6dd9abb676aa6",
            )
            .with_body(include_str!("test_data/blob_indexer_response.json"))
            .create();

        server
    }
}
