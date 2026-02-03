#![allow(clippy::too_many_arguments)]

use alloy::sol;

sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    #[derive(Debug)]
    UserOpsSubmitter,
    "src/l1/abi/UserOpsSubmitter.json"
);

sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    SurgeInbox,
    "src/l1/abi/SurgeInbox.json"
);

sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    #[derive(Debug)]
    Multicall,
    "src/l1/abi/Multicall.json"
);
