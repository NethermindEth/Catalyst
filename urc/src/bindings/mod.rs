use alloy::sol;

sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    IRegistry,
    "src/bindings/abi/IRegistry.json"
);

sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    ILookaheadStore,
    "src/bindings/abi/ILookaheadStore.json"
);
