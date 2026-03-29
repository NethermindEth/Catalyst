use super::l2_block_payload::L2BlockV2Payload;
use anyhow::Error;
use common::l2::taiko_driver::{OperationType, models::BuildPreconfBlockResponse};
use common::shared::l2_slot_info_v2::L2SlotContext;
use std::future::Future;
use std::pin::Pin;

pub trait BlockAdvancer: Send + Sync {
    fn advance_head_to_new_l2_block<'a>(
        &'a self,
        l2_block_payload: L2BlockV2Payload,
        l2_slot_context: &'a L2SlotContext,
        operation_type: OperationType,
    ) -> Pin<Box<dyn Future<Output = Result<BuildPreconfBlockResponse, Error>> + Send + 'a>>;
}
