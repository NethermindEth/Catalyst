#![allow(clippy::too_many_arguments)]

use alloy::sol;

sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    #[derive(Debug)]
    Bridge,
    "src/shared_abi/Bridge.json"
);

sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    #[derive(Debug)]
    SignalService,
    "src/shared_abi/SignalService.json"
);

// HopProof encoding struct for cross-chain signal verification via storage proofs.
// Not part of the SignalService ABI directly — it is the encoding format for the
// `_proof` bytes parameter in proveSignalReceived / verifySignalReceived.
sol! {
    struct HopProof {
        uint64 chainId;
        uint64 blockId;
        bytes32 rootHash;
        uint8 cacheOption;
        bytes[] accountProof;
        bytes[] storageProof;
    }
}
