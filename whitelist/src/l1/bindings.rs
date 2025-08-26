use alloy::sol;

pub mod forced_inclusion_store {
    use super::*;

    sol!(
        #[allow(missing_docs)]
        #[sol(rpc)]
        interface IForcedInclusionStore {
            struct ForcedInclusion {
                bytes32 blobHash;
                uint64 feeInGwei;
                uint64 createdAtBatchId;
                uint32 blobByteOffset;
                uint32 blobByteSize;
                uint64 blobCreatedIn;
            }

            event ForcedInclusionConsumed(ForcedInclusion forcedInclusion);
            event ForcedInclusionStored(ForcedInclusion forcedInclusion);

            function head() external view returns (uint64);
            function tail() external view returns (uint64);
            function getForcedInclusion(uint256 index) external view returns (ForcedInclusion memory);
        }
    );

    impl PartialEq for IForcedInclusionStore::ForcedInclusion {
        fn eq(&self, other: &Self) -> bool {
            self.blobHash == other.blobHash
                && self.feeInGwei == other.feeInGwei
                && self.createdAtBatchId == other.createdAtBatchId
                && self.blobByteOffset == other.blobByteOffset
                && self.blobByteSize == other.blobByteSize
                && self.blobCreatedIn == other.blobCreatedIn
        }
    }
}