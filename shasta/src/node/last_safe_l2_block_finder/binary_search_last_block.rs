use anyhow::Error;

/// Trait for fetching proposal_id for a given block_id
pub trait ProposalIdFetcher {
    async fn get_proposal_id(&self, block_id: u64) -> Result<u64, Error>;
}

/// Binary search to find the last block with target_proposal_id or the last block of previous proposal_id
///
/// # Arguments
/// * `fetcher` - Implementation of ProposalIdFetcher to retrieve proposal_id for blocks
/// * `start_block` - Starting block_id (exclusive, we start from start_block + 1)
/// * `end_block` - Ending block_id (inclusive)
/// * `target_proposal_id` - The proposal_id we're searching for
///
/// # Returns
/// * `Some(block_id)` - The last block with target_proposal_id, or if not found, the last block with proposal_id < target
/// * `None` - If no block found with proposal_id <= target
pub async fn binary_search_last_block<F: ProposalIdFetcher>(
    fetcher: &F,
    start_block: u64,
    end_block: u64,
    target_proposal_id: u64,
) -> Result<Option<u64>, Error> {
    let start = std::time::Instant::now();
    let mut left = start_block + 1;
    let mut right = end_block;
    let mut result = None;

    while left <= right {
        let mid = left + (right - left) / 2;
        let proposal_id = fetcher.get_proposal_id(mid).await?;

        if proposal_id == target_proposal_id {
            result = Some(mid);
            left = mid + 1; // Continue searching right for the last block
        } else if proposal_id < target_proposal_id {
            result = Some(mid); // Keep track of last block with proposal_id < target
            left = mid + 1;
        } else {
            right = mid.saturating_sub(1);
        }
    }

    tracing::debug!(
        "LastSafeL2Block::binary_search_last_block(): Block proposal_id binary search completed in {} ms, found block {:?} for target proposal_id {}",
        start.elapsed().as_millis(),
        result,
        target_proposal_id
    );

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Mock implementation for testing binary search
    struct MockProposalIdFetcher {
        proposal_ids: Vec<u64>,
        call_count: AtomicU32,
    }

    impl MockProposalIdFetcher {
        fn new(proposal_ids: Vec<u64>) -> Self {
            Self {
                proposal_ids,
                call_count: AtomicU32::new(0),
            }
        }

        fn get_call_count(&self) -> u32 {
            self.call_count.load(Ordering::Relaxed)
        }
    }

    impl ProposalIdFetcher for MockProposalIdFetcher {
        async fn get_proposal_id(&self, block_id: u64) -> Result<u64, Error> {
            self.call_count.fetch_add(1, Ordering::Relaxed);

            let idx = block_id as usize;
            if idx < self.proposal_ids.len() {
                Ok(self.proposal_ids[idx])
            } else {
                Err(anyhow::anyhow!("Block id {} out of range", block_id))
            }
        }
    }

    #[tokio::test]
    async fn test_binary_search_exact_match_single() {
        // Proposal IDs: [0, 1, 2, 2, 3]
        //        Block: [0, 1, 2, 3, 4]
        let mock = MockProposalIdFetcher::new(vec![0, 1, 2, 2, 3]);
        let result = binary_search_last_block(&mock, 0, 4, 2).await.unwrap();

        // Should find block 3 (last block with proposal_id 2)
        assert_eq!(result, Some(3));
    }

    #[tokio::test]
    async fn test_binary_search_missing_proposal_id() {
        // Proposal IDs: [0, 1, 1, 1, 2, 2, 2, 3, 3, 3, 5, 5, 6]
        //        Block: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]
        let mock = MockProposalIdFetcher::new(vec![0, 1, 1, 1, 2, 2, 2, 3, 3, 3, 5, 5, 6]);
        let result = binary_search_last_block(&mock, 0, 12, 4).await.unwrap();

        // Should find block 9 (last block with proposal_id 3, since 4 doesn't exist)
        assert_eq!(result, Some(9));
    }

    #[tokio::test]
    async fn test_binary_search_target_at_end() {
        // Proposal IDs: [0, 1, 2, 2, 3, 3, 3]
        //        Block: [0, 1, 2, 3, 4, 5, 6]
        let mock = MockProposalIdFetcher::new(vec![0, 1, 2, 2, 3, 3, 3]);
        let result = binary_search_last_block(&mock, 0, 6, 3).await.unwrap();

        // Should find block 6 (last block with proposal_id 3)
        assert_eq!(result, Some(6));
    }

    #[tokio::test]
    async fn test_binary_search_single_block() {
        // Proposal IDs: [0, 1, 2]
        //        Block: [0, 1, 2]
        let mock = MockProposalIdFetcher::new(vec![0, 1, 2]);
        let result = binary_search_last_block(&mock, 0, 2, 1).await.unwrap();

        assert_eq!(result, Some(1));
    }

    #[tokio::test]
    async fn test_binary_search_all_same_proposal() {
        // Proposal IDs: [0, 2, 2, 2, 2]
        //        Block: [0, 1, 2, 3, 4]
        let mock = MockProposalIdFetcher::new(vec![0, 2, 2, 2, 2]);
        let result = binary_search_last_block(&mock, 0, 4, 2).await.unwrap();

        // Should find last block (4)
        assert_eq!(result, Some(4));
    }

    #[tokio::test]
    async fn test_binary_search_gap_in_sequence() {
        // Proposal IDs: [0, 1, 3, 3, 3, 7, 7, 10]
        //        Block: [0, 1, 2, 3, 4, 5, 6, 7]
        let mock = MockProposalIdFetcher::new(vec![0, 1, 3, 3, 3, 7, 7, 10]);
        let result = binary_search_last_block(&mock, 0, 7, 5).await.unwrap();

        // Should find block 4 (last block with proposal_id 3)
        assert_eq!(result, Some(4));
    }

    #[tokio::test]
    async fn test_binary_search_efficiency() {
        // Test that binary search uses logarithmic number of calls
        // With 1000 elements, should use ~10-11 calls max
        let mut proposal_ids = vec![0; 1000];
        for i in 0..1000 {
            proposal_ids[i] = (i / 10) as u64; // Creates groups of 10 blocks per proposal
        }

        let mock = MockProposalIdFetcher::new(proposal_ids);
        let result = binary_search_last_block(&mock, 0, 999, 50).await.unwrap();

        assert_eq!(result, Some(509));
        // Should use roughly log2(1000) ≈ 10 calls
        let calls = mock.get_call_count();
        assert!(
            calls <= 20,
            "Binary search used {} calls, expected ~10",
            calls
        );
    }

    #[tokio::test]
    async fn test_binary_search_continuous_ids() {
        // Proposal IDs: [0, 1, 2, 3, 4, 5, 6]
        //        Block: [0, 1, 2, 3, 4, 5, 6]
        let mock = MockProposalIdFetcher::new(vec![0, 1, 2, 3, 4, 5, 6]);
        let result = binary_search_last_block(&mock, 0, 6, 4).await.unwrap();

        // Should find block 4 (proposal_id 4)
        assert_eq!(result, Some(4));
    }

    #[tokio::test]
    async fn test_binary_search_multiple_same_at_end() {
        // Proposal IDs: [0, 1, 2, 3, 3, 3, 3]
        //        Block: [0, 1, 2, 3, 4, 5, 6]
        let mock = MockProposalIdFetcher::new(vec![0, 1, 2, 3, 3, 3, 3]);
        let result = binary_search_last_block(&mock, 0, 6, 3).await.unwrap();

        // Should find block 6 (last block with proposal_id 3)
        assert_eq!(result, Some(6));
    }

    #[tokio::test]
    async fn test_binary_search_lower_than_all() {
        // Proposal IDs: [0, 5, 6, 6, 7]
        //        Block: [0, 1, 2, 3, 4]
        let mock = MockProposalIdFetcher::new(vec![0, 5, 6, 6, 7]);
        let result = binary_search_last_block(&mock, 0, 4, 2).await.unwrap();

        // Should return None since all proposal_ids are > target
        assert_eq!(result, None);
    }
}
