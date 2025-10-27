mod bindings;
pub mod execution_layer;
pub mod taiko;

use crate::event_indexer::EventIndexer;
use crate::l2::execution_layer::L2ExecutionLayer;
use alloy::primitives::{Address, B256};
use anyhow::Error;
use common::l2::taiko_driver::{
    OperationType, TaikoDriver,
    models::{BuildPreconfBlockRequestBody, BuildPreconfBlockResponse, ExecutableData},
};
use common::shared::{
    l2_block::L2Block, l2_slot_info::L2SlotInfo, l2_tx_lists::encode_and_compress,
};

#[allow(clippy::too_many_arguments)]
pub async fn advance_head_to_new_l2_block(
    l2_block: L2Block,
    anchor_origin_height: u64,
    anchor_block_state_root: B256,
    anchor_block_hash: B256,
    l2_slot_info: &L2SlotInfo,
    end_of_sequencing: bool,
    is_forced_inclusion: bool,
    operation_type: OperationType,
    driver: &TaikoDriver,
    l2_execution_layer: &L2ExecutionLayer,
    preconfer_address: &Address,
    event_indexer: &EventIndexer,
) -> Result<Option<BuildPreconfBlockResponse>, Error> {
    tracing::debug!(
        "Submitting new L2 block to the Taiko driver with {} txs",
        l2_block.prebuilt_tx_list.tx_list.len()
    );

    // let base_fee_config = l2_execution_layer.get_base_fee_config();
    // let sharing_pctg = base_fee_config.sharingPctg;
    let sharing_pctg = 0; // TODO: read from config

    let propose_input = event_indexer
        .get_propose_input()
        .ok_or(Error::msg("No propose input found"))?;

    let anchor_tx = l2_execution_layer
        .construct_anchor_tx(
            preconfer_address,
            u16::try_from(l2_slot_info.parent_id() + 1)?,
            *l2_slot_info.parent_hash(),
            anchor_origin_height,
            anchor_block_hash,
            anchor_block_state_root,
            l2_slot_info.base_fee(),
            propose_input,
        )
        .await?;
    let tx_list = std::iter::once(anchor_tx)
        .chain(l2_block.prebuilt_tx_list.tx_list.into_iter())
        .collect::<Vec<_>>();

    let tx_list_bytes = encode_and_compress(&tx_list)?;
    let extra_data = vec![sharing_pctg];

    let executable_data = ExecutableData {
        base_fee_per_gas: l2_slot_info.base_fee(),
        block_number: l2_slot_info.parent_id() + 1,
        extra_data: format!("0x{:0>64}", hex::encode(extra_data)),
        fee_recipient: preconfer_address.to_string(),
        gas_limit: 241_000_000u64,
        parent_hash: format!("0x{}", hex::encode(l2_slot_info.parent_hash())),
        timestamp: l2_block.timestamp_sec,
        transactions: format!("0x{}", hex::encode(tx_list_bytes)),
    };

    let request_body = BuildPreconfBlockRequestBody {
        executable_data,
        end_of_sequencing,
        is_forced_inclusion,
    };

    driver.preconf_blocks(request_body, operation_type).await
}
