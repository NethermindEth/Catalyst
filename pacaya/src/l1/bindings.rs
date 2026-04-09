use alloy::sol;

sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    PreconfWhitelist,
    "src/l1/abi/PreconfWhitelist.json"
);

pub mod taiko_inbox {
    use super::*;

    sol!(
        #[allow(missing_docs)]
        #[sol(rpc)]
        ITaikoInbox,
        "src/l1/abi/ITaikoInbox.json"
    );
}
