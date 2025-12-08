use alloy::sol;

sol!(
#[sol(rpc)]
 contract Inbox {
    struct Config {
        /// @notice The codec used for encoding and hashing
        address codec;
        /// @notice The token used for bonds
        address bondToken;
        /// @notice The signal service contract address
        address signalService;
        /// @notice The proof verifier contract
        address proofVerifier;
        /// @notice The proposer checker contract
        address proposerChecker;
        /// @notice The proving window in seconds
        uint48 provingWindow;
        /// @notice The extended proving window in seconds
        uint48 extendedProvingWindow;
        /// @notice The ring buffer size for storing proposal hashes
        uint256 ringBufferSize;
        /// @notice The percentage of basefee paid to coinbase
        uint8 basefeeSharingPctg;
        /// @notice The minimum number of forced inclusions that the proposer is forced to process
        /// if they are due
        uint256 minForcedInclusionCount;
        /// @notice The delay for forced inclusions measured in seconds
        uint16 forcedInclusionDelay;
        /// @notice The base fee for forced inclusions in Gwei used in dynamic fee calculation
        uint64 forcedInclusionFeeInGwei;
        /// @notice Queue size at which the fee doubles
        uint64 forcedInclusionFeeDoubleThreshold;
        /// @notice The minimum delay between checkpoints in seconds
        /// @dev Must be less than or equal to finalization grace period
        uint16 minCheckpointDelay;
        /// @notice The multiplier to determine when a forced inclusion is too old so that proposing
        /// becomes permissionless
        uint8 permissionlessInclusionMultiplier;
    }

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

    function propose(bytes calldata _lookahead, bytes calldata _data) external;

    function getConfig() external view returns (Config memory config_);
}
);
