use alloy::primitives::{B256, Bytes, U256, keccak256};
use anyhow::Error;
use common::shared::l2_slot_info_v2::L2SlotContext;
use common::shared::l2_tx_lists::encode_and_compress;
use common::utils::rpc_client::JSONRPCClient;
use secp256k1::SecretKey;
use ssz_rs::prelude::*;
use std::time::Duration;
use taiko_alethia_reth::validation::ANCHOR_V3_V4_GAS_LIMIT;
use taiko_preconfirmation_driver::rpc::{PreconfSlotInfo, server::METHOD_GET_PRECONF_SLOT_INFO};
use taiko_preconfirmation_driver::rpc::{
    PublishBlockRequest, PublishBlockResponse, server::METHOD_PUBLISH_BLOCK,
};
use taiko_preconfirmation_types::{
    Bytes20, PreconfCommitment, Preconfirmation, SignedCommitment, address_to_bytes20,
    b256_to_bytes32, sign_commitment, u256_to_uint256,
};
use tracing::{debug, trace};
/// Client for communicating with the preconfirmation driver's JSON-RPC server.
///
/// Provides a typed wrapper around the `preconf_getPreconfSlotInfo` RPC method
/// exposed by the preconfirmation driver node.
pub struct PreconfirmationDriver {
    rpc_client: JSONRPCClient,
}

impl PreconfirmationDriver {
    pub fn new_with_timeout(url: &str, timeout: Duration) -> Result<Self, Error> {
        let rpc_client = JSONRPCClient::new_with_timeout(url, timeout)?;
        Ok(Self { rpc_client })
    }

    pub async fn get_preconf_slot_info(&self, timestamp: U256) -> Result<PreconfSlotInfo, Error> {
        trace!("Calling {}", METHOD_GET_PRECONF_SLOT_INFO);
        let response = self
            .rpc_client
            .call_method(
                METHOD_GET_PRECONF_SLOT_INFO,
                vec![serde_json::to_value(timestamp)?],
            )
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "preconfirmation driver: {} RPC call failed: {}",
                    METHOD_GET_PRECONF_SLOT_INFO,
                    e
                )
            })?;
        let slot_info: PreconfSlotInfo = serde_json::from_value(response)?;
        Ok(slot_info)
    }

    /// Function to publish a Signed Preconfirmation Commitment and a Transaction List
    pub async fn post_preconf_requests(
        &self,
        l2_slot_context: &L2SlotContext,
        tx_list: &[alloy::rpc::types::Transaction],
        coinbase: alloy::primitives::Address,
        anchor_block_id: u64,
        signer_key: &SecretKey,
    ) -> Result<PublishBlockResponse, Error> {
        let timestamp_sec = l2_slot_context.info.slot_timestamp();
        let tx_list_bytes = encode_and_compress(tx_list)?;
        let tx_list_hash = keccak256(&tx_list_bytes);
        let submission_window_end = self
            .get_preconf_slot_info(U256::from(timestamp_sec))
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "preconfirmation driver: {} failed for timestamp_sec={}: {}",
                    METHOD_GET_PRECONF_SLOT_INFO,
                    timestamp_sec,
                    e
                )
            })?
            .submission_window_end;
        let preconf = Preconfirmation {
            eop: l2_slot_context.end_of_sequencing,
            block_number: u256_to_uint256(U256::from(l2_slot_context.info.parent_id() + 1)),
            timestamp: u256_to_uint256(U256::from(timestamp_sec)),
            gas_limit: u256_to_uint256(U256::from(
                l2_slot_context.info.parent_gas_limit_without_anchor() + ANCHOR_V3_V4_GAS_LIMIT,
            )),
            coinbase: address_to_bytes20(coinbase),
            anchor_block_number: u256_to_uint256(U256::from(anchor_block_id)),
            raw_tx_list_hash: b256_to_bytes32(tx_list_hash),
            parent_preconfirmation_hash: b256_to_bytes32(B256::ZERO),
            submission_window_end: u256_to_uint256(submission_window_end),
            prover_auth: Bytes20::default(),
            proposal_id: u256_to_uint256(U256::from(l2_slot_context.info.parent_id() + 1)),
        };

        let commitment = PreconfCommitment {
            preconf,
            slasher_address: Bytes20::default(),
        };

        let signature = sign_commitment(&commitment, signer_key)?;
        let signed_commitment = SignedCommitment {
            commitment,
            signature,
        };
        let signed_commitment_bytes = ssz_rs::serialize(&signed_commitment)
            .map_err(|e| anyhow::anyhow!("SSZ Serialization Failed: {:?}", e))?;

        let request = PublishBlockRequest {
            commitment: Bytes::from(signed_commitment_bytes),
            tx_list_hash,
            tx_list: Bytes::from(tx_list_bytes),
        };
        self.publish_block(request).await.map_err(|e| {
            anyhow::anyhow!(
                "preconfirmation driver: {} RPC call failed: {}",
                METHOD_PUBLISH_BLOCK,
                e
            )
        })
    }

    async fn publish_block(&self, req: PublishBlockRequest) -> Result<PublishBlockResponse, Error> {
        debug!("Calling {}", METHOD_PUBLISH_BLOCK);
        let response = self
            .rpc_client
            .call_method(METHOD_PUBLISH_BLOCK, vec![serde_json::to_value(req)?])
            .await?;
        let block_response: PublishBlockResponse = serde_json::from_value(response)?;
        Ok(block_response)
    }
}
