//! Traits for batch-like structures that can be used with common batch builder logic.

use crate::shared::l2_block::L2Block;

/// Trait for types that represent a batch or proposal containing L2 blocks.
///
/// This trait abstracts over `Proposal` (shasta) and `Batch` (pacaya) to allow
/// common batch builder logic to work with both types.
pub trait BatchLike: Clone {
    /// Returns a mutable reference to the L2 blocks in this batch.
    fn l2_blocks_mut(&mut self) -> &mut Vec<L2Block>;

    /// Returns a reference to the L2 blocks in this batch.
    fn l2_blocks(&self) -> &Vec<L2Block>;

    /// Returns a mutable reference to the total bytes count.
    fn total_bytes_mut(&mut self) -> &mut u64;

    /// Returns the total bytes count.
    fn total_bytes(&self) -> u64;

    /// Returns the anchor block ID.
    fn anchor_block_id(&self) -> u64;

    /// Returns the anchor block timestamp in seconds.
    fn anchor_block_timestamp_sec(&self) -> u64;

    /// Compresses the batch, updating the total_bytes field.
    fn compress(&mut self);
}
