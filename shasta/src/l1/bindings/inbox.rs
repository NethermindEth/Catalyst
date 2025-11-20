use alloy::sol;

sol!(
#[sol(rpc)]
 contract Inbox {
    struct BlobSlice {
        /// @notice The blobs containing the proposal's content.
        bytes32[] blobHashes;
        /// @notice The byte offset of the proposal's content in the containing blobs.
        uint24 offset;
        /// @notice The timestamp when the frame was created.
        uint48 timestamp;
    }

    struct ForcedInclusion {
        uint64 feeInGwei;
        BlobSlice blobSlice;
    }

    uint48 public activationTimestamp;

    function getForcedInclusionState()
        external
        view
        returns (uint48 head_, uint48 tail_, uint48 lastProcessedAt_);

    function getForcedInclusions(
            uint48 _start,
            uint48 _maxCount
        )
            external
            view
            returns (ForcedInclusion[] memory inclusions_);
});
