use alloy::sol;

sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    IRegistry,
    "src/l1/abi/IRegistry.json"
);
