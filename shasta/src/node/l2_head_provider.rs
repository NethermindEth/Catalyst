use crate::{l1::execution_layer::ExecutionLayer, l2::taiko::Taiko};
use anyhow::Error;
use common::l1::ethereum_l1::EthereumL1;
use std::sync::Arc;

pub async fn get_l2_height_from_l1(
    ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
    taiko: Arc<Taiko>,
) -> Result<u64, Error> {
    let proposal_id = ethereum_l1
        .execution_layer
        .get_proposal_id_from_indexer()
        .await?;
    taiko
        .l2_execution_layer()
        .get_last_block_by_proposal(proposal_id)
        .await
}
