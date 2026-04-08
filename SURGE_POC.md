# Surge Real-Time Composability POC

This document explains the end-to-end flow of the Surge real-time composability proof-of-concept. It covers how a UserOp enters the system, gets simulated, triggers cross-chain calls, and lands on L1 — all within a single proposal.

## Architecture Overview

```
Client                  Node                           L1 Chain
  │                      │                                │
  │── surge_sendUserOp ─▶│                                │
  │◀── returns op ID ────│                                │
  │                      │                                │
  │                      │── simulate UserOp (trace) ───▶│
  │                      │◀── MessageSent + SignalSent ──│
  │                      │                                │
  │                      │── build L2 block ──┐           │
  │                      │   (anchor with     │           │
  │                      │    signal slots +   │           │
  │                      │    bridge processMessage)      │
  │                      │◀───────────────────┘           │
  │                      │                                │
  │                      │── detect L2→L1 calls           │
  │                      │                                │
  │                      │── build L1 multicall: ────────▶│
  │                      │   1. execute UserOp             │
  │                      │   2. propose batch (blobs)      │
  │                      │   3. relay L1 call              │
  │                      │                                │
  │── surge_userOpStatus▶│                                │
  │◀── Executed ─────────│                                │
```

## Key Idea

A single L1 multicall transaction atomically:
1. **Executes the UserOp** on L1 (e.g. bridge deposit)
2. **Proposes the batch** containing L2 blocks that already processed the L2 side of that bridge call
3. **Relays any L2→L1 callbacks** from those L2 blocks

The L2 blocks can process the bridge message immediately because the anchor transaction sets the signal slots ahead of time — before the L1 tx even lands. If the L1 multicall reverts, the L2 blocks are also invalid, maintaining consistency.

## Flow Step by Step

### 1. UserOp Submission (RPC)

A client sends a UserOp via the `surge_sendUserOp` JSON-RPC endpoint.

- **File**: [bridge_handler.rs:131](shasta/src/node/proposal_manager/bridge_handler.rs#L131)
- The RPC server assigns a unique ID, persists `Pending` status to a [sled store](shasta/src/node/proposal_manager/bridge_handler.rs#L37), and pushes the UserOp into an mpsc channel
- Returns the ID immediately so the client can poll status

```jsonc
// Request
{ "method": "surge_sendUserOp", "params": [{ "submitter": "0x...", "calldata": "0x..." }] }
// Response
{ "result": 1 }  // UserOp ID
```

**Status polling** via [`surge_userOpStatus`](shasta/src/node/proposal_manager/bridge_handler.rs#L159):
```jsonc
{ "method": "surge_userOpStatus", "params": [1] }
// Response cycles through: Pending → Processing { tx_hash } → Executed / Rejected { reason }
```

### 2. UserOp Simulation on L1

During block building, the [BatchManager](shasta/src/node/proposal_manager/mod.rs#L40) polls the BridgeHandler for pending UserOps.

- **Entry**: [add_pending_l2_call_to_draft_block](shasta/src/node/proposal_manager/mod.rs#L293)
- **Calls**: [next_user_op_and_l2_call](shasta/src/node/proposal_manager/bridge_handler.rs#L210)
- **Which calls**: [find_message_and_signal_slot (L1)](shasta/src/l1/execution_layer.rs#L380)

The L1 execution layer simulates the UserOp using `debug_trace_call` with a call tracer. It [recursively collects logs](shasta/src/l1/execution_layer.rs#L359) from the call tree looking for:
- **`MessageSent`** — the bridge message (from the Bridge contract)
- **`SignalSent`** — the signal slot (from the SignalService contract)

If both are found, the UserOp is valid. If not, it's [rejected immediately](shasta/src/node/proposal_manager/bridge_handler.rs#L233).

### 3. L2 Block Construction

With a valid UserOp, the system builds an L2 block containing:

1. **Anchor transaction** with signal slots — [construct_anchor_tx](shasta/src/l2/execution_layer.rs#L86) calls `anchorV4WithSignalSlots(checkpoint, signalSlots)`. This sets the signal slots on L2 so the bridge message can be validated immediately.

2. **Bridge processMessage transaction** — [construct_l2_call_tx](shasta/src/l2/execution_layer.rs#L295) builds a signed tx calling `bridge.processMessage(message, proof)` on L2.

Both are inserted into the draft block in [add_draft_block_to_proposal](shasta/src/node/proposal_manager/mod.rs#L331), which also records the UserOp and signal slot on the [Proposal](shasta/src/node/proposal_manager/proposal.rs#L16).

After the L2 block is produced, the system [detects any L2→L1 calls](shasta/src/node/proposal_manager/mod.rs#L379) by querying L2 logs via [find_message_and_signal_slot (L2)](shasta/src/l2/execution_layer.rs#L357). Found calls get a signed proof and are added as [L1Call](shasta/src/node/proposal_manager/bridge_handler.rs#L78) to the proposal.

### 4. L1 Multicall Submission

When the batch is ready, [try_submit_oldest_batch](shasta/src/node/proposal_manager/batch_builder.rs#L346) triggers submission.

The [ProposalTxBuilder](shasta/src/l1/proposal_tx_builder.rs#L33) assembles the multicall in [build_propose_blob](shasta/src/l1/proposal_tx_builder.rs#L99):

| Order | Call | Purpose |
|-------|------|---------|
| 1 | [build_user_op_call](shasta/src/l1/proposal_tx_builder.rs#L166) | Execute the UserOp on L1 (e.g. bridge deposit) |
| 2 | `build_propose_call` | Propose the batch with compressed block data as blobs |
| 3 | [build_l1_call_call](shasta/src/l1/proposal_tx_builder.rs#L241) | Relay L2→L1 bridge callback with signed proof |

All three are bundled into one multicall tx sent to the `proposer_multicall` contract. This is atomic — if any call fails, all revert.

The tx is sent via [send_batch_to_l1](shasta/src/l1/execution_layer.rs#L185) which passes it to the [TransactionMonitor](common/src/shared/transaction_monitor.rs#L41).

### 5. Transaction Monitoring & Status Updates

The [TransactionMonitor](common/src/shared/transaction_monitor.rs#L97) handles gas bumping, resubmission, and confirmation tracking. Two oneshot channels notify the batch builder:

1. **tx_hash_notifier** — fires when the tx is first sent → status becomes `Processing { tx_hash }`
2. **tx_result_notifier** — fires when the tx is confirmed or fails → status becomes `Executed` or `Rejected`

This is orchestrated by a [background task](shasta/src/node/proposal_manager/batch_builder.rs#L346) spawned during submission.

### 6. Main Loop

The [Node](shasta/src/node/mod.rs#L34) runs a preconfirmation loop via [entrypoint](shasta/src/node/mod.rs#L113). Each tick of [main_block_preconfirmation_step](shasta/src/node/mod.rs#L157):

1. Checks if the node is the active preconfer
2. Checks if there are pending transactions OR [pending UserOps](shasta/src/node/proposal_manager/mod.rs#L283)
3. Calls [preconfirm_block](shasta/src/node/proposal_manager/mod.rs#L130) to build a block
4. Calls [try_submit_oldest_batch](shasta/src/node/proposal_manager/mod.rs#L104) to submit when ready

## Key Files

| File | Role |
|------|------|
| [bridge_handler.rs](shasta/src/node/proposal_manager/bridge_handler.rs) | RPC server, UserOp intake, L1/L2 call detection, status store |
| [mod.rs](shasta/src/node/proposal_manager/mod.rs) | BatchManager — orchestrates block building with bridge data |
| [batch_builder.rs](shasta/src/node/proposal_manager/batch_builder.rs) | Accumulates blocks into proposals, handles submission + status tracking |
| [proposal.rs](shasta/src/node/proposal_manager/proposal.rs) | Proposal struct with Surge fields (user_ops, signal_slots, l1_calls, checkpoint) |
| [execution_layer.rs (L1)](shasta/src/l1/execution_layer.rs) | L1 simulation (trace), batch submission |
| [execution_layer.rs (L2)](shasta/src/l2/execution_layer.rs) | L2 tx construction (anchor, bridge call), L2 event detection |
| [proposal_tx_builder.rs](shasta/src/l1/proposal_tx_builder.rs) | Assembles the L1 multicall (UserOp + propose + L1 call) |
| [transaction_monitor.rs](common/src/shared/transaction_monitor.rs) | L1 tx lifecycle — send, bump gas, confirm, notify |
| [node/mod.rs](shasta/src/node/mod.rs) | Main preconfirmation loop |
| [lib.rs](shasta/src/lib.rs) | Node startup and initialization |

## Configuration

| Env Var / Constant | Where | What |
|---|---|---|
| Bridge RPC address | [mod.rs:77](shasta/src/node/proposal_manager/mod.rs#L77) | Hardcoded `127.0.0.1:4545` |
| Status DB path | [bridge_handler.rs:116](shasta/src/node/proposal_manager/bridge_handler.rs#L116) | Hardcoded `data/user_op_status` |
| L1 call proof signer | [bridge_handler.rs:193](shasta/src/node/proposal_manager/bridge_handler.rs#L193) | Anvil key #0 (POC only) |
| Preconf heartbeat | Node config | `PRECONF_HEARTBEAT_MS` (default 2000ms) |

## Tweaking the POC

**Change what the UserOp does**: Modify [build_user_op_call](shasta/src/l1/proposal_tx_builder.rs#L166) — currently it just forwards `submitter` + `calldata` as-is into the multicall.

**Change how L2 processes the bridge message**: Modify [construct_l2_call_tx](shasta/src/l2/execution_layer.rs#L295) — currently calls `bridge.processMessage()`.

**Change the multicall order or add calls**: Modify [build_propose_blob](shasta/src/l1/proposal_tx_builder.rs#L99) — the order of calls in the multicall array.

**Change how L1 simulation extracts events**: Modify [find_message_and_signal_slot (L1)](shasta/src/l1/execution_layer.rs#L380) — change which events are extracted from the trace.

**Change status persistence**: Swap out [UserOpStatusStore](shasta/src/node/proposal_manager/bridge_handler.rs#L37) — currently backed by sled at `data/user_op_status`.

**Change the RPC port**: Update the socket address in [BatchManager::new](shasta/src/node/proposal_manager/mod.rs#L77).
