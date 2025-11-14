use alloy::sol;

sol!(
#[sol(rpc)]
 contract Inbox {
    uint48 public activationTimestamp;
});
