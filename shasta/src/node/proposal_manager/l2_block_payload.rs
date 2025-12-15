use alloy::primitives::B256;
use alloy::rpc::types::Transaction;

pub struct L2BlockV2Payload {
    pub proposal_id: u64,
    pub coinbase: alloy::primitives::Address,
    pub tx_list: Vec<Transaction>,
    pub timestamp_sec: u64,
    pub gas_limit_without_anchor: u64,
    pub anchor_block_id: u64,
    pub anchor_block_hash: B256,
    pub anchor_state_root: B256,
    pub is_forced_inclusion: bool,
}
