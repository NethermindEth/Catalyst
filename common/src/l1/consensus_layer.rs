use std::{error::Error as std_error, time::Duration};

use alloy::rpc::types::beacon::sidecar::BeaconBlobBundle;
use anyhow::Error;
use reqwest;

pub struct ConsensusLayer {
    client: reqwest::Client,
    url: reqwest::Url,
}

impl ConsensusLayer {
    pub fn new(rpc_url: &str, timeout: Duration) -> Result<Self, Error> {
        if !rpc_url.ends_with('/') {
            return Err(anyhow::anyhow!("Consensus layer URL must end with '/'"));
        }
        let client = reqwest::Client::builder().timeout(timeout).build()?;
        Ok(Self {
            client,
            url: reqwest::Url::parse(rpc_url)?,
        })
    }

    pub async fn get_blob_sidecars(&self, slot: u64) -> Result<BeaconBlobBundle, Error> {
        let res = self
            .get(format!("eth/v1/beacon/blob_sidecars/{slot}").as_str())
            .await?;
        let sidecar: BeaconBlobBundle = serde_json::from_value(res)?;
        Ok(sidecar)
    }

    pub async fn get_genesis_time(&self) -> Result<u64, Error> {
        tracing::debug!("Getting genesis time");
        let genesis = self.get("eth/v1/beacon/genesis").await?;
        let genesis_time = genesis
            .get("data")
            .and_then(|data| data.get("genesis_time"))
            .and_then(|genesis_time| genesis_time.as_str())
            .ok_or_else(|| {
                anyhow::anyhow!("get_genesis_time error: missing or invalid 'genesis_time' field")
            })?
            .parse::<u64>()
            .map_err(|err| anyhow::anyhow!("get_genesis_time error: {}", err))?;
        Ok(genesis_time)
    }

    pub async fn get_head_slot_number(&self) -> Result<u64, Error> {
        let headers = self.get("eth/v1/beacon/headers/head").await?;

        let slot = headers
            .get("data")
            .and_then(|data| data.get("header"))
            .and_then(|header| header.get("message"))
            .and_then(|message| message.get("slot"))
            .and_then(|slot| slot.as_str())
            .ok_or(anyhow::anyhow!(
                "get_head_slot_number error: {}",
                "slot is not a string"
            ))?
            .parse::<u64>()
            .map_err(|err| anyhow::anyhow!("get_head_slot_number error: {}", err))?;
        Ok(slot)
    }

    pub async fn get_validators_for_epoch(&self, epoch: u64) -> Result<Vec<String>, Error> {
        let response = self
            .get(format!("eth/v1/validator/duties/proposer/{epoch}").as_str())
            .await?;

        let validators_response = response
            .get("data")
            .ok_or(anyhow::anyhow!(
                "get_validators_for_epoch invalid response body: {}",
                "`data` not found"
            ))?
            .as_array()
            .ok_or(anyhow::anyhow!(
                "get_validators_for_epoch invalid response body: {}",
                "`data` is not an array"
            ))?;

        let mut validators = Vec::with_capacity(32);
        for validator_response in validators_response {
            // This public key is received in the compressed form
            let pubkey = validator_response
                .get("pubkey")
                .and_then(|pubkey| pubkey.as_str())
                .ok_or(anyhow::anyhow!(
                    "get_validators_for_epoch invalid response body: {}",
                    "array element does not contain `pubkey`"
                ))?;

            validators.push(pubkey.to_string());
        }

        Ok(validators)
    }

    async fn get(&self, path: &str) -> Result<serde_json::Value, Error> {
        let response = self
            .client
            .get(self.url.join(path)?)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    anyhow::anyhow!("Consensus layer request timed out: {}", path)
                } else {
                    anyhow::anyhow!(
                        "Consensus layer request failed with error: {}. Source: {:?}",
                        e,
                        e.source()
                    )
                }
            })?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "Consensus layer request ({}) failed with status: {}",
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
    use tokio;

    #[tokio::test]
    async fn test_get_genesis_data() {
        let server = setup_server().await;
        let cl = ConsensusLayer::new(
            format!("{}/", server.url()).as_str(),
            Duration::from_secs(1),
        )
        .unwrap();
        let genesis_time = cl.get_genesis_time().await.unwrap();

        assert_eq!(genesis_time, 1590832934);
    }

    #[tokio::test]
    async fn test_get_head_slot_number() {
        let server = setup_server().await;
        let cl = ConsensusLayer::new(
            format!("{}/", server.url()).as_str(),
            Duration::from_secs(1),
        )
        .unwrap();
        let slot = cl.get_head_slot_number().await.unwrap();

        assert_eq!(slot, 4269575);
    }

    async fn setup_server() -> mockito::ServerGuard {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/eth/v1/beacon/genesis")
            .with_body(r#"{
                "data": {
                  "genesis_time": "1590832934",
                  "genesis_validators_root": "0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2",
                  "genesis_fork_version": "0x00000000"
                }
              }"#)
            .create();
        server
            .mock("GET", "/eth/v1/validator/duties/proposer/1")
            .with_body(include_str!("test_data/lookahead_test_response.json"))
            .create();

        server
            .mock("GET", "/eth/v1/beacon/headers/head")
            .with_body(r#"
            {"execution_optimistic":false,"finalized":false,"data":{"root":"0xc6cab6f6378b6027b16230ef30e696e5c5784e25d2808f1a143e533af2fac604","canonical":true,"header":{"message":{"slot":"4269575","proposer_index":"1922844","parent_root":"0x9b742c3e4f1b7670b5b36c2739dea8823e547abcba8e84a84e9cfc75598eec88","state_root":"0x2be9f894881f8cf30db3aeacbff9ac4a1a17bd2045e9427b790a2d8ea6a2a884","body_root":"0xe3919dd62dca9c81e515c9f0c6b13210cf80cf02b1f1a0e54234e17571f20451"},"signature":"0xb2dd797184a46515266707cc01ee48a313d5d3723dc883d5d3a311a124f21a24a0920ed40fdfca854999de3fb01c26d80314f3100504ba11f388a112243bba5e3a40c0ec2b9bfb8e31a9933e1ffc5d05f25083aef6c497144a6cda90438a90c4"}}}
            "#)
            .create();

        server
    }

    #[test]
    fn test_join_path() {
        let base =
            reqwest::Url::parse("https://lb.drpc.test/eth-beacon-chain-hoodi/oasijdfjoasfmd/")
                .unwrap();
        let url = base.join("eth/v1/beacon/genesis").unwrap();
        assert_eq!(
            url.as_str(),
            "https://lb.drpc.test/eth-beacon-chain-hoodi/oasijdfjoasfmd/eth/v1/beacon/genesis"
        );
    }
}
