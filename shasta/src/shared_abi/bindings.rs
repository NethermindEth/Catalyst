#![allow(clippy::too_many_arguments)]

use alloy::sol;

sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    #[derive(Debug)]
    Bridge,
    "src/shared_abi/Bridge.json"
);

// SignalSent event emitted by the SignalService contract
sol! {
    #[allow(missing_docs)]
    event SignalSent(address indexed app, bytes32 signal, bytes32 slot, bytes32 value);
}
