use crate::l1::protocol_config::ProtocolConfig;
use crate::l2::execution_layer::L2ExecutionLayer;
use crate::node::proposal_manager::block_advancer::BlockAdvancer;
use crate::node::proposal_manager::l2_block_payload::L2BlockV2Payload;
use anyhow::Error;
use common::l2::taiko_driver::{
    OperationType, TaikoDriver,
    models::{BuildPreconfBlockRequestBody, BuildPreconfBlockResponse, ExecutableData},
};
use common::shared::l2_slot_info_v2::L2SlotContext;
use common::shared::l2_tx_lists;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use taiko_alethia_reth::validation::ANCHOR_V3_V4_GAS_LIMIT;
use taiko_bindings::anchor::Anchor;

pub struct ShastaBlockAdvancer {
    l2_execution_layer: Arc<L2ExecutionLayer>,
    protocol_config: ProtocolConfig,
    driver: Arc<TaikoDriver>,
}

impl ShastaBlockAdvancer {
    pub fn new(
        l2_execution_layer: Arc<L2ExecutionLayer>,
        protocol_config: ProtocolConfig,
        driver: Arc<TaikoDriver>,
    ) -> Self {
        Self {
            l2_execution_layer,
            protocol_config,
            driver,
        }
    }
}

impl BlockAdvancer for ShastaBlockAdvancer {
    fn advance_head_to_new_l2_block<'a>(
        &'a self,
        l2_block_payload: L2BlockV2Payload,
        l2_slot_context: &'a L2SlotContext,
        operation_type: OperationType,
    ) -> Pin<Box<dyn Future<Output = Result<BuildPreconfBlockResponse, Error>> + Send + 'a>> {
        Box::pin(async move {
            tracing::debug!(
                "Submitting new L2 block to the Taiko driver with {} txs",
                l2_block_payload.tx_list.len()
            );

            let anchor_block_params = Anchor::BlockParams {
                anchorBlockNumber: l2_block_payload.anchor_block_id.try_into()?,
                anchorBlockHash: l2_block_payload.anchor_block_hash,
                anchorStateRoot: l2_block_payload.anchor_state_root,
                rawTxListHash: Default::default(),
            };

            let anchor_tx = self
                .l2_execution_layer
                .construct_anchor_tx(&l2_slot_context.info, anchor_block_params)
                .await
                .map_err(|e| {
                    anyhow::anyhow!(
                        "advance_head_to_new_l2_block: Failed to construct anchor tx: {}",
                        e
                    )
                })?;
            let tx_list = std::iter::once(anchor_tx)
                .chain(l2_block_payload.tx_list)
                .collect::<Vec<_>>();

            let tx_list_bytes = l2_tx_lists::encode_and_compress(&tx_list)?;

            let sharing_pctg = self.protocol_config.get_basefee_sharing_pctg();
            let extra_data = crate::l2::extra_data::ExtraData {
                basefee_sharing_pctg: sharing_pctg,
                proposal_id: l2_block_payload.proposal_id,
            }
            .encode()
            .map_err(|e| {
                anyhow::anyhow!(
                    "advance_head_to_new_l2_block: Failed to encode extra data: {}",
                    e
                )
            })?;

            let executable_data = ExecutableData {
                base_fee_per_gas: l2_slot_context.info.base_fee(),
                block_number: l2_slot_context.info.parent_id() + 1,
                extra_data: format!("0x{}", hex::encode(extra_data)),
                fee_recipient: l2_block_payload.coinbase.to_string(),
                gas_limit: l2_block_payload.gas_limit_without_anchor + ANCHOR_V3_V4_GAS_LIMIT,
                parent_hash: format!("0x{}", hex::encode(l2_slot_context.info.parent_hash())),
                timestamp: l2_block_payload.timestamp_sec,
                transactions: format!("0x{}", hex::encode(tx_list_bytes)),
            };

            let request_body = BuildPreconfBlockRequestBody {
                executable_data,
                end_of_sequencing: l2_slot_context.end_of_sequencing,
                is_forced_inclusion: l2_block_payload.is_forced_inclusion,
            };

            self.driver
                .preconf_blocks(request_body, operation_type)
                .await
        })
    }
}
