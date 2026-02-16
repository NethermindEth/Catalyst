use crate::{l1::execution_layer::ExecutionLayer, l2::taiko::Taiko};
use anyhow::Error;
use common::l1::ethereum_l1::EthereumL1;
use std::sync::Arc;

pub async fn get_l2_height_from_l1(
    ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
    taiko: Arc<Taiko>,
) -> Result<u64, Error> {
    let inbox_state = ethereum_l1.execution_layer.get_inbox_state().await?;
    if inbox_state.nextProposalId == 1 {
        taiko.l2_execution_layer().get_head_l1_origin().await.or_else(|_| {
            tracing::warn!("Failed to get L2 head from get_head_l1_origin, but nextProposalId is 1, so returning L2 height as 0");
            Ok(0u64) // If no proposals have been made, we can consider the L2 height to be 0
        })
    } else {
        tracing::debug!("Fetching L2 height from L1 nextProposalId: {}", inbox_state.nextProposalId);
        taiko.get_last_block_id_by_batch_id(inbox_state.nextProposalId.to::<u64>() - 1).await
    }
}
