# Permissionless Implementation

## Current Status

The permissionless implementation has been suspended. The latest implementation features include preconfirmation of L2 transactions, which are published using the preconfirmation-driver, and the posting of Shasta proposals, which successfully landed on the L1 chain.

## Repositories/branches

- alethia-reth, anchorV4 and signature encoding modifications added
  - branch: https://github.com/mskrzypkows/alethia-reth/tree/permissionless
- taiko-mono, rebased `permissionless-preconfs/scripts` branch with latest main changes, corrected merge conflicts
  - docker image: `nethswitchboard/taiko-client-rs:permissionless`
  - branch: https://github.com/taikoxyz/taiko-mono/tree/permissionless-preconfs_client-changes
- taiko-geth, corrected encoding of BatchID field
  - docker image: `nethswitchboard/preconf-taiko-geth:permissionless`
  - branch: https://github.com/mskrzypkows/taiko-geth/tree/permissionless
- catalyst, added usage of modified bindings
  - docker image: `nethswitchboard/catalyst-node:permissionless`
  - branch: https://github.com/NethermindEth/Catalyst/tree/permissionless
- protocol, commented out permissionless checks
  - docker image: `nethswitchboard/preconf-taiko-protocol:permissionless`
  - branch: https://github.com/taikoxyz/taiko-mono/tree/permissionless-preconfs/scripts with commented (in Inbox.sol):
    ```
                // if (!result.allowsPermissionless) {
                //     endOfSubmissionWindowTimestamp =
                //         _proposerChecker.checkProposer(msg.sender, _lookahead);
                //     require(_bondStorage.hasSufficientBond(msg.sender, _minBond), InsufficientBond());
                // }
    ```
- devnet, added usage for permissionless-driver
  - branch: https://github.com/NethermindEth/simple-taiko-node-nethermind/tree/catalyst_pacaya_shasta-urc

## Constraints API

We were exploring the use of https://github.com/eth-fabric/fabric/commit/11052eb5941a0ae21dd30c161c4146e51a38c1a1 to enable forced inclusion into L1 blocks.

During testing, we identified a potential issue in rbuilder. When the constraint spammer is stopped, the constraint-server logs show that rbuilder does not retrieve constraints for every block. As a result, constraints are only processed for some blocks instead of consistently for each one.

```
ERROR tokio-runtime-worker ThreadId(12) Failed to submit blocks with proofs: No signed constraints found for slot 43
ERROR tokio-runtime-worker ThreadId(12) Failed to submit blocks with proofs: No signed constraints found for slot 46 
ERROR tokio-runtime-worker ThreadId(12) Failed to submit blocks with proofs: No signed constraints found for slot 49
```