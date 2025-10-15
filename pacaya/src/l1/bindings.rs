use alloy::sol;

sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    PreconfWhitelist,
    "src/l1/abi/PreconfWhitelist.json"
);

sol! {
    /// @dev Represents proposeBlock's _data input parameter
    struct BlockParamsV2 {
        address proposer;
        address coinbase;
        bytes32 parentMetaHash;
        uint64 anchorBlockId; // NEW
        uint64 timestamp; // NEW
        uint32 blobTxListOffset; // NEW
        uint32 blobTxListLength; // NEW
        uint8 blobIndex; // NEW
    }
}

sol! {
    // https://github.com/NethermindEth/preconf-taiko-mono/blob/main/packages/protocol/contracts/layer1/based/ITaikoInbox.sol
    struct BlockParams {
        // the max number of transactions in this block. Note that if there are not enough
        // transactions in calldata or blobs, the block will contains as many transactions as
        // possible.
        uint16 numTransactions;
        // For the first block in a batch,  the block timestamp is the batch params' `timestamp`
        // plus this time shift value;
        // For all other blocks in the same batch, the block timestamp is its parent block's
        // timestamp plus this time shift value.
        uint8 timeShift;
        // Signals sent on L1 and need to sync to this L2 block.
        bytes32[] signalSlots;
    }

    struct BlobParams {
        // The hashes of the blob. Note that if this array is not empty.  `firstBlobIndex` and
        // `numBlobs` must be 0.
        bytes32[] blobHashes;
        // The index of the first blob in this batch.
        uint8 firstBlobIndex;
        // The number of blobs in this batch. Blobs are initially concatenated and subsequently
        // decompressed via Zlib.
        uint8 numBlobs;
        // The byte offset of the blob in the batch.
        uint32 byteOffset;
        // The byte size of the blob.
        uint32 byteSize;
        // The block number when the blob was created.
        uint64 createdIn;
    }

    struct BatchParams {
        address proposer;
        address coinbase;
        bytes32 parentMetaHash;
        uint64 anchorBlockId;
        uint64 lastBlockTimestamp;
        bool revertIfNotFirstProposal;
        // Specifies the number of blocks to be generated from this batch.
        BlobParams blobParams;
        BlockParams[] blocks;
    }

    struct ProposeBatchWrapper {
        bytes bytesX;
        bytes bytesY;
    }
}

pub mod taiko_inbox {
    use super::*;

    sol!(
        #[allow(missing_docs)]
        #[sol(rpc)]
        ITaikoInbox,
        "src/l1/abi/ITaikoInbox.json"
    );
}

sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    PreconfRouter,
    "src/l1/abi/PreconfRouter.json"
);

pub mod preconf_router {
    use super::*;

    sol!(
        #[allow(missing_docs)]
        #[sol(rpc)]
        interface IPreconfRouter is IProposeBatch {
            error ForcedInclusionNotSupported();
            error NotPreconferOrFallback();
            error ProposerIsNotPreconfer();

            struct Config {
                uint256 handOverSlots;
            }

            function getConfig() external view returns (Config memory);
        }
    );
}

pub mod taiko_wrapper {
    use super::*;

    sol!(
        #[allow(missing_docs)]
        #[sol(rpc)]
        TaikoWrapper,
        "src/l1/abi/TaikoWrapper.json"
    );
}

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
