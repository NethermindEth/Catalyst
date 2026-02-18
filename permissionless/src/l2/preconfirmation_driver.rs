use alloy::primitives::{B256, Bytes, U256, keccak256};
use anyhow::Error;
use common::shared::l2_slot_info_v2::L2SlotContext;
use common::shared::l2_tx_lists::encode_and_compress;
use common::utils::rpc_client::JSONRPCClient;
use secp256k1::SecretKey;
use shasta::L2BlockV2Payload;
use ssz_rs::prelude::*;
use std::time::Duration;
use taiko_alethia_reth::validation::ANCHOR_V3_V4_GAS_LIMIT;
use taiko_preconfirmation_driver::rpc::{PreconfSlotInfo, server::METHOD_GET_PRECONF_SLOT_INFO};
use taiko_preconfirmation_driver::rpc::{
    PublishCommitmentRequest, PublishCommitmentResponse, PublishTxListRequest,
    PublishTxListResponse,
    server::{METHOD_PUBLISH_COMMITMENT, METHOD_PUBLISH_TX_LIST},
};
use taiko_preconfirmation_types::{
    Bytes20, PreconfCommitment, Preconfirmation, SignedCommitment, address_to_bytes20,
    b256_to_bytes32, sign_commitment, u256_to_uint256,
};
use tracing::debug;
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
        debug!("Calling {}", METHOD_GET_PRECONF_SLOT_INFO);
        let response = self
            .rpc_client
            .call_method(
                METHOD_GET_PRECONF_SLOT_INFO,
                vec![serde_json::to_value(timestamp)?],
            )
            .await?;
        let slot_info: PreconfSlotInfo = serde_json::from_value(response)?;
        Ok(slot_info)
    }

    /// Function to publish a Signed Preconfirmation Commitment and a Transaction List
    #[allow(dead_code)]
    pub async fn post_preconf_requests(
        &self,
        l2_block_payload: L2BlockV2Payload,
        l2_slot_context: &L2SlotContext,
        signer_key: &SecretKey,
    ) -> Result<(PublishTxListResponse, PublishCommitmentResponse), Error> {
        let tx_list = &l2_block_payload.tx_list;
        let tx_list_bytes = encode_and_compress(tx_list)?;
        let tx_list_hash = keccak256(&tx_list_bytes);
        let submission_window_end = self
            .get_preconf_slot_info(U256::from(l2_block_payload.timestamp_sec))
            .await?
            .submission_window_end;
        let preconf = Preconfirmation {
            eop: l2_slot_context.end_of_sequencing,
            block_number: u256_to_uint256(U256::from(l2_slot_context.info.parent_id() + 1)),
            timestamp: u256_to_uint256(U256::from(l2_block_payload.timestamp_sec)),
            gas_limit: u256_to_uint256(U256::from(
                l2_block_payload.gas_limit_without_anchor + ANCHOR_V3_V4_GAS_LIMIT,
            )),
            coinbase: address_to_bytes20(l2_block_payload.coinbase),
            anchor_block_number: u256_to_uint256(U256::from(l2_block_payload.anchor_block_id)),
            raw_tx_list_hash: b256_to_bytes32(tx_list_hash),
            parent_preconfirmation_hash: b256_to_bytes32(B256::ZERO),
            submission_window_end: u256_to_uint256(submission_window_end),
            prover_auth: Bytes20::default(),
            proposal_id: u256_to_uint256(U256::from(l2_block_payload.proposal_id)),
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

        let tx_request = PublishTxListRequest {
            tx_list_hash,
            tx_list: Bytes::from(tx_list_bytes),
        };
        let commitment_request = PublishCommitmentRequest {
            commitment: Bytes::from(signed_commitment_bytes),
        };
        let tx_response = self.publish_tx_list(tx_request).await?;
        let commitment_response = self.publish_commitment(commitment_request).await?;
        Ok((tx_response, commitment_response))
    }

    async fn publish_tx_list(
        &self,
        req: PublishTxListRequest,
    ) -> Result<PublishTxListResponse, Error> {
        debug!("Calling {}", METHOD_PUBLISH_TX_LIST);
        let response = self
            .rpc_client
            .call_method(METHOD_PUBLISH_TX_LIST, vec![serde_json::to_value(req)?])
            .await?;
        let tx_list_response: PublishTxListResponse = serde_json::from_value(response)?;
        Ok(tx_list_response)
    }

    async fn publish_commitment(
        &self,
        req: PublishCommitmentRequest,
    ) -> Result<PublishCommitmentResponse, Error> {
        debug!("Calling {}", METHOD_PUBLISH_COMMITMENT);
        let response = self
            .rpc_client
            .call_method(METHOD_PUBLISH_COMMITMENT, vec![serde_json::to_value(req)?])
            .await?;
        let commitment_response: PublishCommitmentResponse = serde_json::from_value(response)?;
        Ok(commitment_response)
    }
}
