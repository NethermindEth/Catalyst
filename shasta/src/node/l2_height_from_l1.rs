use crate::{l1::execution_layer::ExecutionLayer, l2::taiko::Taiko};
use anyhow::{Error, anyhow};
use common::l1::ethereum_l1::EthereumL1;
use std::sync::Arc;

pub async fn get_l2_height_from_l1(
    ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
    taiko: Arc<Taiko>,
) -> Result<u64, Error> {
    match taiko.l2_execution_layer().get_head_l1_origin().await {
        Ok(height) => Ok(height),
        Err(err) => {
            // On error, check next_proposal_id from inbox state
            // If it's 1, it means no proposals have been made yet, so L2 height is 0
            tracing::warn!("Failed to get L2 head from get_head_l1_origin: {}", err);
            let inbox_state = ethereum_l1.execution_layer.get_inbox_state().await?;
            if inbox_state.nextProposalId == 1 {
                Ok(0)
            } else {
                Err(anyhow!(
                    "Failed to get L2 head from get_head_l1_origin, next_proposal_id = {}",
                    inbox_state.nextProposalId
                ))
            }
        }
    }
}
