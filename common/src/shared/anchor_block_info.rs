use crate::shared::execution_layer::ExecutionLayer;
use alloy::primitives::B256;
use anyhow::Error;

pub struct AnchorBlockInfo {
    id: u64,
    timestamp_sec: u64,
    hash: B256,
    state_root: B256,
}

impl AnchorBlockInfo {
    pub async fn from_lag(
        execution_layer: &ExecutionLayer,
        l1_height_lag: u64,
    ) -> Result<Self, Error> {
        let id = Self::calculate_anchor_block_id(execution_layer, l1_height_lag).await?;
        Self::from_block_number(execution_layer, id).await
    }

    pub async fn from_precomputed_data(
        execution_layer: &ExecutionLayer,
        id: u64,
        hash: B256,
        state_root: B256,
    ) -> Result<Self, Error> {
        let timestamp_sec = execution_layer.get_block_timestamp_by_number(id).await?;
        Ok(Self {
            id,
            timestamp_sec,
            hash,
            state_root,
        })
    }

    pub async fn from_block_number(
        execution_layer: &ExecutionLayer,
        number: u64,
    ) -> Result<Self, Error> {
        let timestamp_sec = execution_layer
            .get_block_timestamp_by_number(number)
            .await?;
        let hash = execution_layer.get_block_hash(number).await?;
        let state_root = execution_layer
            .get_block_state_root_by_number(number)
            .await?;
        Ok(Self {
            id: number,
            timestamp_sec,
            hash,
            state_root,
        })
    }

    async fn calculate_anchor_block_id(
        execution_layer: &ExecutionLayer,
        l1_height_lag: u64,
    ) -> Result<u64, Error> {
        let l1_height = execution_layer.get_latest_block_id().await?;
        let l1_height_with_lag = l1_height - l1_height_lag;

        Ok(l1_height_with_lag)
    }

    pub fn id(&self) -> u64 {
        self.id
    }
    pub fn timestamp_sec(&self) -> u64 {
        self.timestamp_sec
    }
    pub fn hash(&self) -> B256 {
        self.hash
    }
    pub fn state_root(&self) -> B256 {
        self.state_root
    }
}
