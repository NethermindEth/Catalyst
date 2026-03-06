use crate::l2::preconfirmation_driver::PreconfirmationDriver;
use alloy::primitives::{Address, B256};
use anyhow::Error;
use common::l2::taiko_driver::{OperationType, models::BuildPreconfBlockResponse};
use common::shared::l2_slot_info_v2::L2SlotContext;
use secp256k1::SecretKey;
use shasta::{BlockAdvancer, L2BlockV2Payload, l2::taiko::Taiko};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use taiko_bindings::anchor::Anchor;
use tracing::info;

pub struct PermissionlessBlockAdvancer {
    preconfirmation_driver: Arc<PreconfirmationDriver>,
    taiko: Arc<Taiko>,
    coinbase: Address,
    signer_key: SecretKey,
}

impl PermissionlessBlockAdvancer {
    pub fn new(
        preconfirmation_driver: Arc<PreconfirmationDriver>,
        taiko: Arc<Taiko>,
        coinbase: Address,
        signer_key: SecretKey,
    ) -> Self {
        Self {
            preconfirmation_driver,
            taiko,
            coinbase,
            signer_key,
        }
    }
}

impl BlockAdvancer for PermissionlessBlockAdvancer {
    fn advance_head_to_new_l2_block<'a>(
        &'a self,
        l2_block_payload: L2BlockV2Payload,
        l2_slot_context: &'a L2SlotContext,
        _operation_type: OperationType,
    ) -> Pin<Box<dyn Future<Output = Result<BuildPreconfBlockResponse, Error>> + Send + 'a>> {
        Box::pin(async move {
            let anchor_block_params = Anchor::BlockParams {
                anchorBlockNumber: l2_block_payload.anchor_block_id.try_into()?,
                anchorBlockHash: l2_block_payload.anchor_block_hash,
                anchorStateRoot: l2_block_payload.anchor_state_root,
                rawTxListHash: Default::default(),
            };

            let anchor_tx = self
                .taiko
                .l2_execution_layer()
                .construct_anchor_tx(&l2_slot_context.info, anchor_block_params)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to construct anchor tx: {}", e))?;

            let tx_list = std::iter::once(anchor_tx)
                .chain(l2_block_payload.tx_list)
                .collect::<Vec<_>>();

            let response = self
                .preconfirmation_driver
                .post_preconf_requests(
                    l2_slot_context,
                    &tx_list,
                    self.coinbase,
                    l2_block_payload.anchor_block_id,
                    &self.signer_key,
                )
                .await
                .map_err(|e| anyhow::anyhow!("Failed to post preconfirmation requests: {}", e))?;

            info!(
                "Published preconfirmation: tx_list= {}, commitment= {}",
                response.tx_list_hash, response.commitment_hash
            );

            Ok(BuildPreconfBlockResponse {
                number: l2_slot_context.info.parent_id() + 1,
                hash: B256::ZERO, // TODO: missing hash from the response, do we need it for permissionless?
                parent_hash: *l2_slot_context.info.parent_hash(),
                is_forced_inclusion: l2_block_payload.is_forced_inclusion,
            })
        })
    }
}
