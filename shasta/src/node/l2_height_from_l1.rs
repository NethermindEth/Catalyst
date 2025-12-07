use crate::{l1::execution_layer::ExecutionLayer, l2::taiko::Taiko};
use anyhow::Error;
use common::l1::ethereum_l1::EthereumL1;
use std::sync::Arc;

pub async fn get_l2_height_from_l1(
    _ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
    taiko: Arc<Taiko>,
) -> Result<u64, Error> {
    match taiko.l2_execution_layer().get_head_l1_origin().await {
        Ok(height) => Ok(height),
        Err(err) => {
            // On error, check proposal_id
            // TODO fix this logic properly
            tracing::error!("Failed to get L2 head from get_head_l1_origin: {}", err);
            Ok(0)
            /*let proposal_id = ethereum_l1
                .execution_layer
                .get_proposal_id_from_indexer()
                .await?;

            if proposal_id == 0 { Ok(0) } else { Err(err) }*/
        }
    }
}
