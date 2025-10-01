# (Oct 2024) Taiko Preconf PoC Progress Report

In this doc, we will go through what we have implemented for the PoC, and what needs to be further researched and implemented for mainnet release.

# Slides

[https://docs.google.com/presentation/d/1-v1j6ub069fyugvDZSNRrNwVDl1LnQwSI1Y2jgF_Uy0/edit?usp=sharing](https://docs.google.com/presentation/d/1-v1j6ub069fyugvDZSNRrNwVDl1LnQwSI1Y2jgF_Uy0/edit?usp=sharing)

# Current Status

We implemented all features presented in [the design doc](https://github.com/NethermindEth/Taiko-Preconf-AVS/blob/master/docs/design-doc.md). In this section we will briefly summarize the key features; for more details of each feature, refer to the design doc or [our repository](https://github.com/NethermindEth/Taiko-Preconf-AVS).

## Smart Contracts

On the contract side ****([code](https://github.com/NethermindEth/Taiko-Preconf-AVS/tree/master/SmartContracts)), we have:

- A **preconfirmation registry** contract that enables validators to opt-in to becoming preconfers, with a validator BLS key to ECDSA mapping scheme, which reduces gas costs by 1/3 compared to designs that use BLS keys for signing the block proposals.
- A **lookahead submission scheme** with an optimistic fraud-proof mechanism.
- A **fraud-proof mechanism** for slashing equivocation in preconfirmations.
- A **generic restaking support** that enables integration with any restaking protocol following the EigenLayer interfaces, such as EigenLayer itself (once they properly support slashing), [Karak](https://docs.karak.network/), or custom staking contracts.

## AVS Node

On the AVS node side ([code](https://github.com/NethermindEth/Taiko-Preconf-AVS/tree/master/node)), we fully automate:

- **Validator registration** to the preconfirmation registry at initial setup.
- **Lookahead** submissions and disputes.
- **Dispute** **against preconfirmations** made by other validators.
- Execution of the **main preconfirmation duties**, which include:
    - Checking the lookahead to determine if it's the validator’s turn to preconfirm.
    - Constructing L2 blocks using the Taiko mempool.
    - Publishing the L2 block to a preconfirmation P2P network.
    - Force-including the preconfirmed L2 blocks to L1 using [Chainbound’s Bolt](https://chainbound.github.io/bolt-docs/).
    - Syncing the local Taiko head with the latest preconfirmation state.

Furthermore, implementing some of the AVS functionality required changes to the Taiko client, specifically for fetching transactions from the Taiko mempool and for advancing the Taiko head. You can find the [list of commits in our taiko-mono fork](https://github.com/taikoxyz/taiko-mono/compare/main...NethermindEth:taiko-mono:main).

## Infra

We have deployment scripts that enable setting up an E2E testing environment. With a single command, it will automatically:

- Spin up a devnet with 8 validators total
    - 4 Validators as 2 different preconfers
    - 4 Validators not registered as proconfers
- Deploys Taiko contracts + AVS contracts
- Registers preconfer and validators
- Deploys Bolt's mev booost and Bolt's builder that supports it.
- Deploys 2 AVS Nodes (one for each preconfer) connected to 2 separate Taiko stack nodes.
- Runs a Tx Spammer for Taiko ready to spam the mempool with transactions to be picked up by the preconfer.

Refer to the [README of our kurtosis package](https://github.com/NethermindEth/preconfirm-devnet-package/blob/main/README.md) for more details on how to run the E2E test.

# Towards Mainnet

In this section, we outline the necessary next steps to bring the PoC implementation to mainnet. These steps broadly fall into two categories:

- Implementing the **required features** into the PoC.
- Planning a **release plan** for mainnet.

Note that this section is a result of early-stage exploration. One of the first steps toward the mainnet is to flesh out the steps mentioned here, identify any missing ones, concretely document the design choices, and organize the tasks based on priority.

## Required Features

We will outline the required features to be incorporated into the PoC to make it mainnet-ready.

### Multi-block Blob Support

**Problem:**

A recent fork of Taiko's contracts added support for including multiple L2 blocks within a single blob. However, our PoC implementation relies on an earlier version before this multi-block support (i.e., we use [BlockParams](https://github.com/NethermindEth/taiko-mono/blob/c2f59c30c085f19c5fed64e07c7961009060c428/packages/protocol/contracts/layer1/based/TaikoData.sol#L64) instead of [BlockParamsV2](https://github.com/NethermindEth/taiko-mono/blob/c2f59c30c085f19c5fed64e07c7961009060c428/packages/protocol/contracts/layer1/based/TaikoData.sol#L73)).

Supporting multi-block would require reconsideration of design, most notably around preconfirmation slashing. As outlined in [our design doc](https://github.com/NethermindEth/Taiko-Preconf-AVS/blob/master/docs/design-doc.md#incorrect-preconfirmation-slashing), our current slashing logic ([code](https://github.com/NethermindEth/Taiko-Preconf-AVS/blob/ca2ce61682ff58a5b105ec8e5626112cf45a1094/SmartContracts/src/avs/PreconfTaskManager.sol#L122)) works by comparing the `txListHash` of the preconfirmed block with that of the proposed block. However, with the introduction of multi-block support, the hash of individual proposed blocks is no longer easily accessible on-chain. This is because the Taiko inbox now stores only the blob hash of the entire blob—which contains multiple blocks—along with offset information to introspect the blob.

**Potential Solution(s):**

- Instead of slashing by comparing the whole tx list hash (or blob hash), we slash by comparing specific bytes within the tx list. E.g., Disputes look like “the preconfer preconfed an L2 block that has `txListBytes[42]=0xAB`, but the proposed blob had `blobBytes[block offset within blob + 42]=0xFF`". This would require each preconfirmation to include some commitment (likely KZG?) to the individual L2 blocks.
- Wait until the L2 tx list is settled in L1, then utilize the settled metadata of the individual L2 blocks for slashing.

### Proper Support of Prover Bonds

**Problem:**

In the latest version of the Taiko inbox, the assigned prover is hard-coded to be the `msg.sender` of `TaikoL1.sol` ([related PR](https://github.com/taikoxyz/taiko-mono/pull/17553)). This is problematic since, with a preconf protocol, the `msg.sender` will be the preconf AVS contract instead of the proposer itself. As a result:

- The liveness bond is accounted for from our AVS contract, [PreconfTaskManager.sol](https://github.com/NethermindEth/Taiko-Preconf-AVS/blob/ca2ce61682ff58a5b105ec8e5626112cf45a1094/SmartContracts/src/avs/PreconfTaskManager.sol#L111), instead of the proposer.
- The proof must be submitted by the PreconfTaskManager, instead of the proposer.

**Potential Solution(s):**

- Keep the current Taiko inbox contract, and:
    - Do accounting for per-proposer liveness bonds within PreconfTaskManager.
    - Enable submitting proof through the PreconfTaskManager.
- Modify the Taiko inbox contract to decouple the proposer from the prover (e.g., by requiring a “signed permit” from the prover).

### Fair Exchange

**Problem:**

Currently, nothing incentivizes nor enforces the preconfer to release preconfirmations in a timely manner.

**Potential Solution(s):**

- Combine a transaction expiry mechanism that allows users to specify by when they want their transaction to be preconfirmed (e.g., “by 2nd sub-slot of slot X”), together with an end-user monitoring functionality where wallets or Taiko full nodes in which the user is connected to stops sending order flow to the mempool once they detect preconf withholding. Refer to the [multi-round MEV-Boost post](https://ethresear.ch/t/based-preconfirmations-with-multi-round-mev-boost/20091#protocol-description-5) for more details on this approach.
- Require the preconf software to run in a TEE that enforces the timely releases.
- [Introduce VDSs](https://research.chainbound.io/exploring-verifiable-continuous-sequencing-with-delay-functions)
- Have some committee monitor the timely releases of the preconfirmations, and let them kick out (or even slash) preconfers that are withholding preconfs. At the initial bootstrapping phase, the “committee” can potentially be a set of whitelisted guardians. Then we can eventually decentralize or move to a different solution.

### Restaking Protocol Integration and Testing

**Problem:**

Our PoC integrates with the EigenLayer interfaces but not EigenLayer itself. For the implementation, we implemented our own “MVP” version of EigenLayer as a placeholder. This is because:

- EigenLayer is missing many functionalities to be functional, such as slashing.
- We wanted to avoid tight coupling with EigenLayer.

However, for mainnet release, we should test out integration with actual restaking solutions, while continuing to avoid the coupling with a specific protocol.

**Potential Solution(s):**

- Test out multiple restaking solutions such as EigenLayer, [Karak](https://docs.karak.network/), or custom staking contracts if any, and let validators choose whatever solution they prefer.

### Improve lookahead security

**Problem:**

In the PoC, the first preconfirmer in the lookahead is tasked with submitting the lookahead for the next epoch and is slashed if they submit an invalid one ([doc](https://github.com/NethermindEth/Taiko-Preconf-AVS/blob/master/docs/design-doc.md#lookahead-visibility)). However, if the profit from proposing Taiko blocks becomes higher than the slashing risk, this preconfirmer might submit an invalid lookahead to unfairly elect themselves as the preconfirmer for the entire next epoch.

**Solution:**

Subsequent preconfirmers can either:

- Attest to the initial lookahead, accepting the risk of slashing if the lookahead is invalid.
- Submit an alternative lookahead by staking additional funds (`C*X`, where `C` is the previous submitter stake and X is a multiplier) upon detecting an invalid submission by the first preconfirmer.

By the end of the epoch, the lookahead will have either:

- Attestations from all preconfirmers (say there are `N`) of the previous epoch, or
- `C*X^N` worth of stake from the final submitter.

Or some combination of the two.

```
S1 ---------------------------S2--------------..............------------S32---|
^                             ^                                          ^
|                             |                                          |
proposer                   Either propose                             The lookahead
submits                    an alternative                             of next epoch
invalid                    lookahead with                             has EITHER
lookahead                  C*X stake                                  C*X^N stake
for next epoch             OR                                         backing it
with C stake               "Attest" to the                            where N is num
                           previously submitted                       of preconfer in
                           lookahead and be exposed to                current epoch
                           slashing of C stake.                       OR
                                                                      N preconfers
                                                                      attestations.
```


### Other Features

There are several other issues we would want to consider for the mainnet release, including but not limited to:

- **Sync to Latest Taiko Contracts:** The PoC is branched off of an old version of Taiko contracts. We should update our AVS contracts and node to adapt to the latest Taiko contracts.
- **Slashing amount**: In the PoC, we require a fixed 1 ETH deposit per preconfirmer ECDSA key. Note that each ECDSA key may support multiple validators through our BLS→ECDSA mapping in the registry, so this means this 1 ETH deposit can support arbitrary number of validators. Further consideration is needed regarding the slashing amount.
    - **Potential solution:** When deciding the slashing amount, incorporate factors such as the number of validators or the number of elected slots associated with the preconfirmer.
- **Better ECDSA key management:** A common complaint from node operators about using ECDSA for preconfirmation duties is the lack of support for distributed key management. In contrast, BLS signatures allow keys to be split into multiple parts, enabling threshold signing schemes.
    - **Potential solution**: Introduce BLS→multi-sig ECDSA to enable distributed key management. Since the gas cost of `ECRECOVER` is [3000 gas](https://www.evm.codes/precompiled), on-chain verification of the multi-sig will be significantly lower than BLS signature checks.
- **Hardening lookahead submissions and disputes**: In the PoC, the first preconfirmer in the lookahead is tasked with submitting the lookahead for the next epoch and is slashed if they submit an invalid one. However, if the profit from proposing Taiko blocks becomes high, this preconfirmer might submit an invalid lookahead to unfairly elect themselves as the preconfirmer for the entire next epoch.
    - **Potential solution**: Allow subsequent preconfirmers to submit an alternative lookahead by staking additional funds when they detect that the first preconfirmer posted an invalid lookahead.
- **Proper Stake Lock:** EigenLayer currently lacks a “state lock” feature where we can disable withdrawals during the dispute period. They plan to implement them, but they do not have a clear interface defined yet, so we left such a state lock out of our PoC.
    - **Potential solution**: Wait until EigenLayer defines its interface.
    - **Alternative:** Rely on some [“withdrawal delay”](https://docs.eigenlayer.xyz/eigenlayer/security/withdrawal-delay) mechanism of the underlying restaking protocol.
- **Preconf release timing:** In the PoC, the preconfed L2 blocks are released at the `0s, 3s, 6s, 9s` mark. This is nice as it gives us `3s` after the final L2 block preconf to propagate the preconfed L2 blocks to the builder via Bolt MEV-Boost inclusion lists. However, this requires the preconfer to release its first block at the beginning of its slot, where it might not have enough time to construct L2 blocks from the L2 mempool (especially if the previous preconfer withheld its final L2 block from preconf P2P).
    - **Potential solution**: Consider `3s, 6s, 9s, 12s-x`, where `x` is the time required to propagate the inclusion list and is set based on concrete measurements.
- **Fallback to Taiko Proposer:** Right now, when there is no preconfer in the lookahead, we fall back to random selection from opted in preconfers. An alternative would be to fall back to the Taiko proposer instead.
- **Punish Inactive Preconfers:** If a preconfer opts-in but does not propose any L2 blocks in their slot, kick them out of the preconfer set.

## Release Plan

This section outlines potential paths for integrating with actual mainnet actors for release.

An example iterative release plan can be:

- Disable opt-in preconfers and set Taiko as default fallback.
- Enable opt-in preconfers via white list.
- Enable opt-in preconfers with Taiko-controlled black list.
- Remove black list.

At the same time, we can add PBS on top of the preconf solution:

- Ahead-of-time delegation to external preconfers using the BLS->ECDSA mapping.
- JIT auction of preconf L2 blocks via Taiko-specific MEV-Boost.
- Integration with L1 MEV-Boost to enable L1-L2 composability.
