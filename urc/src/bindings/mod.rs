use alloy::sol;

sol! {
    #[allow(missing_docs)]
    #[sol(rpc)]
    IRegistry,
    "src/bindings/abi/Registry.json"
}
