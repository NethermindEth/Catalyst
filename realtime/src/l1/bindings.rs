#![allow(clippy::too_many_arguments)]

use alloy::sol;

sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    #[derive(Debug, Default)]
    RealTimeInbox,
    "src/l1/abi/RealTimeInbox.json"
);

sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    #[derive(Debug)]
    Multicall,
    "src/l1/abi/Multicall.json"
);

// Define ProposeInput and BlobReference manually since the RealTimeInbox ABI
// only exposes propose(bytes _data, ...) where _data is abi.encode(ProposeInput).
// These types are internal to the contract but needed for encoding.
sol! {
    struct BlobReference {
        uint16 blobStartIndex;
        uint16 numBlobs;
        uint24 offset;
    }

    struct ProposeInput {
        BlobReference blobReference;
        bytes32[] signalSlots;
        uint48 maxAnchorBlockNumber;
    }
}
