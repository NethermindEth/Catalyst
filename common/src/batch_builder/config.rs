use alloy::primitives::Address;

/// Configuration for batching L2 transactions
#[derive(Clone)]
pub struct BatchBuilderConfig {
    /// Maximum size of the batch in bytes before sending
    pub max_bytes_size_of_batch: u64,
    /// Maximum number of blocks in a batch
    pub max_blocks_per_batch: u16,
    /// L1 slot duration in seconds
    pub l1_slot_duration_sec: u64,
    /// Maximum time shift between blocks in seconds
    pub max_time_shift_between_blocks_sec: u64,
    /// The max differences of the anchor height and the current block number
    pub max_anchor_height_offset: u64,
    /// Default coinbase
    pub default_coinbase: Address,
    /// Minimum number of transactions in a preconfirmed block
    pub preconf_min_txs: u64,
    /// Maximum number of skipped slots in a preconfirmed block
    pub preconf_max_skipped_l2_slots: u64,
}

impl BatchBuilderConfig {
    pub fn is_within_block_limit(&self, num_blocks: u16) -> bool {
        num_blocks <= self.max_blocks_per_batch
    }

    pub fn is_within_bytes_limit(&self, total_bytes: u64) -> bool {
        total_bytes <= self.max_bytes_size_of_batch
    }
}
