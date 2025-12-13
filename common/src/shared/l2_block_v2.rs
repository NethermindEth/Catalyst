use crate::shared::l2_tx_lists::PreBuiltTxList;
use alloy::primitives::Address;

#[derive(Debug, Clone)]
pub struct L2BlockV2Draft {
    pub prebuilt_tx_list: PreBuiltTxList,
    pub timestamp_sec: u64,
    pub gas_limit_without_anchor: u64,
}

#[derive(Debug, Clone)]
pub struct L2BlockV2 {
    pub prebuilt_tx_list: PreBuiltTxList,
    pub timestamp_sec: u64,
    pub coinbase: Address,
    pub anchor_block_number: u64,
    pub gas_limit_without_anchor: u64,
}

impl L2BlockV2 {
    pub fn new_from(
        tx_list: PreBuiltTxList,
        timestamp_sec: u64,
        coinbase: Address,
        anchor_block_number: u64,
        gas_limit_without_anchor: u64,
    ) -> Self {
        L2BlockV2 {
            prebuilt_tx_list: tx_list,
            timestamp_sec,
            coinbase,
            anchor_block_number,
            gas_limit_without_anchor,
        }
    }

    pub fn new_empty(
        timestamp_sec: u64,
        coinbase: Address,
        anchor_block_number: u64,
        gas_limit_without_anchor: u64,
    ) -> Self {
        L2BlockV2 {
            prebuilt_tx_list: PreBuiltTxList::empty(),
            timestamp_sec,
            coinbase,
            anchor_block_number,
            gas_limit_without_anchor,
        }
    }
}
