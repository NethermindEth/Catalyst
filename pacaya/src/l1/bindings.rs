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
