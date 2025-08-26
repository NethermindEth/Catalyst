use alloy::rpc::types::Transaction;

// TODO move to the whitelist forced_inclusion module
pub struct ForcedInclusionInfo {
    pub blob_hash: alloy::primitives::B256,
    pub blob_byte_offset: u32,
    pub blob_byte_size: u32,
    pub created_in: u64,
    pub txs: Vec<Transaction>,
}
