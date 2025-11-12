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
    pub async fn from_chain_state(
        execution_layer: &ExecutionLayer,
        l1_height_lag: u64,
        last_anchor_id: u64,
        min_anchor_offset: u64,
    ) -> Result<Self, Error> {
        let id = Self::calculate_anchor_block_id(
            execution_layer,
            l1_height_lag,
            last_anchor_id,
            min_anchor_offset,
        )
        .await?;
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
        let block_info = execution_layer.get_block_info_by_number(number).await?;
        Ok(Self {
            id: number,
            timestamp_sec: block_info.timestamp,
            hash: block_info.hash,
            state_root: block_info.state_root,
        })
    }

    async fn calculate_anchor_block_id(
        execution_layer: &ExecutionLayer,
        l1_height_lag: u64,
        last_anchor_id: u64,
        min_anchor_offset: u64,
    ) -> Result<u64, Error> {
        let l1_height = execution_layer.get_latest_block_id().await?;
        let l1_height_with_lag = l1_height - l1_height_lag;

        let anchor_id = l1_height_with_lag.max(last_anchor_id + 1);

        if l1_height < anchor_id + min_anchor_offset {
            return Err(anyhow::anyhow!(
                "Calculated anchor block ID {} exceeds latest L1 height {} - min_anchor_offset {}",
                anchor_id,
                l1_height,
                min_anchor_offset
            ));
        }

        Ok(anchor_id)
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
