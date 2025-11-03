#![allow(clippy::too_many_arguments)]

use alloy::sol;

sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    BondManager,
    "src/l2/abi/BondManager.json"
);

sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    Bridge,
    "src/l2/abi/Bridge.json"
);
