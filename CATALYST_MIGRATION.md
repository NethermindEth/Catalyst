# Catalyst Real-Time Fork Migration Plan

> Step-by-step plan to migrate Catalyst from the asynchronous Shasta proving model to the
> single-phase **RealTimeInbox** (atomic propose+prove) model.

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Current Architecture (Shasta)](#2-current-architecture-shasta)
3. [Target Architecture (RealTime)](#3-target-architecture-realtime)
4. [Migration Strategy](#4-migration-strategy)
5. [Step 1 — Scaffold the `realtime` Crate](#step-1--scaffold-the-realtime-crate)
6. [Step 2 — Contract Bindings & ABIs](#step-2--contract-bindings--abis)
7. [Step 3 — Configuration & Environment](#step-3--configuration--environment)
8. [Step 4 — Protocol Config Adapter](#step-4--protocol-config-adapter)
9. [Step 5 — Proposal Struct Changes](#step-5--proposal-struct-changes)
10. [Step 6 — Raiko Proof Client](#step-6--raiko-proof-client)
11. [Step 7 — Proposal Transaction Builder](#step-7--proposal-transaction-builder)
12. [Step 8 — L1 Execution Layer](#step-8--l1-execution-layer)
13. [Step 9 — L2 Anchor Transaction](#step-9--l2-anchor-transaction)
14. [Step 10 — Node Main Loop](#step-10--node-main-loop)
15. [Step 11 — Batch Manager / Proposal Manager](#step-11--batch-manager--proposal-manager)
16. [Step 12 — Remove Dead Code](#step-12--remove-dead-code)
17. [Step 13 — Integration Testing](#step-13--integration-testing)
18. [Appendix A — File Mapping (Shasta → RealTime)](#appendix-a--file-mapping-shasta--realtime)
19. [Appendix B — Environment Variable Changes](#appendix-b--environment-variable-changes)
20. [Appendix C — Raiko API Quick Reference](#appendix-c--raiko-api-quick-reference)

---

## 1. Executive Summary

**Shasta** (current) uses a two-phase model: proposals are submitted to L1 and later proven
by an external prover. Catalyst preconfirms blocks, batches them, and submits via
`SurgeInbox.proposeWithProof()` where the "proof" is a signed checkpoint (161-byte signature).

**RealTime** (target) collapses propose + prove into a single atomic transaction.
Before submitting to L1, the sequencer must:
1. Execute the L2 blocks locally.
2. Request a ZK proof from Raiko covering those blocks.
3. Submit the proposal + ZK proof to `RealTimeInbox.propose()` in one tx.

### Key Differences

| Aspect | Shasta | RealTime |
|---|---|---|
| L1 Contract | `Inbox` (SurgeInbox fork) | `RealTimeInbox` |
| Propose function | `proposeWithProof(data, input, proof, signalSlots)` | `propose(data, checkpoint, proof)` |
| Proof type | Signed checkpoint (161 bytes) | ZK proof from Raiko |
| Prove phase | Separate (external prover) | Embedded in propose tx |
| State tracking | Ring buffer, `CoreState`, proposal IDs | Single `lastProposalHash` |
| Bonds / forced inclusions | Yes | No |
| Batch size | Multiple proposals per proof | Exactly 1 proposal per proof |
| L2 Anchor | `anchorV4WithSignalSlots` | `anchorV4WithSignalSlots` (same) |
| Proposal identification | Sequential `id` | Hash-based (`proposalHash`) |

---

## 2. Current Architecture (Shasta)

### Data Flow

```
L2 Txs → preconfirm_block() → BatchBuilder → Proposal → ProposalTxBuilder
                                                              ↓
                                                    build_propose_call()
                                                              ↓
                                                    sign checkpoint (161-byte proof)
                                                              ↓
                                                    Multicall { user_ops, propose, l1_calls }
                                                              ↓
                                                    EIP-4844 blob tx → L1
```

### Key Files

| Component | Path |
|---|---|
| Entry point | `shasta/src/lib.rs` |
| Node loop | `shasta/src/node/mod.rs` |
| Proposal Manager | `shasta/src/node/proposal_manager/mod.rs` |
| Batch Builder | `shasta/src/node/proposal_manager/batch_builder.rs` |
| Proposal struct | `shasta/src/node/proposal_manager/proposal.rs` |
| TX Builder | `shasta/src/l1/proposal_tx_builder.rs` |
| L1 Execution Layer | `shasta/src/l1/execution_layer.rs` |
| L1 Bindings | `shasta/src/l1/bindings.rs` |
| L1 Config | `shasta/src/l1/config.rs` |
| Protocol Config | `shasta/src/l1/protocol_config.rs` |
| L2 Execution Layer | `shasta/src/l2/execution_layer.rs` |
| L2 Anchor Bindings | `shasta/src/l2/bindings.rs` |
| Forced Inclusion | `shasta/src/forced_inclusion/mod.rs` |
| Bridge Handler | `shasta/src/node/proposal_manager/bridge_handler.rs` |
| Utils / Config | `shasta/src/utils/config.rs` |

### Current Proof Flow (Shasta)

In `ProposalTxBuilder::build_proof_data()` (proposal_tx_builder.rs:148-162):
```rust
// 1. ABI-encode the checkpoint (blockNumber, blockHash, stateRoot)
let checkpoint_encoded = checkpoint.abi_encode();           // 96 bytes
// 2. Keccak hash and sign with hardcoded anvil key
let checkpoint_digest = keccak256(&checkpoint_encoded);
let signature = self.checkpoint_signer.sign_hash(&checkpoint_digest).await?;
// 3. Concatenate: [96-byte checkpoint || 65-byte signature] = 161 bytes
```

This is submitted as the `proof` parameter to `SurgeInbox.proposeWithProof()`.

---

## 3. Target Architecture (RealTime)

### Data Flow

```
L2 Txs → preconfirm_block() → ProposalManager → Proposal
                                                      ↓
                                              finalize proposal (checkpoint known)
                                                      ↓
                                              Request ZK proof from Raiko
                                                  (poll until ready)
                                                      ↓
                                              build_propose_call()
                                                      ↓
                                              Multicall { user_ops, propose, l1_calls }
                                                      ↓
                                              EIP-4844 blob tx → L1
```

### RealTimeInbox Contract Interface

```solidity
function propose(
    bytes calldata _data,                           // abi.encode(ProposeInput)
    ICheckpointStore.Checkpoint calldata _checkpoint,
    bytes calldata _proof                           // ZK proof from Raiko
) external;

function getLastProposalHash() external view returns (bytes32);
function getConfig() external view returns (Config memory);
// Config = { proofVerifier, signalService, basefeeSharingPctg }
```

### Proof Verification On-Chain

```
commitmentHash = keccak256(abi.encode(
    proposalHash,           // keccak256(abi.encode(Proposal))
    checkpoint.blockNumber,
    checkpoint.blockHash,
    checkpoint.stateRoot
))

verifyProof(0, commitmentHash, proof)
```

---

## 4. Migration Strategy

The `realtime` crate will be a **separate crate** alongside `shasta`, sharing `common` and
workspace dependencies. Code will be copied from shasta and modified — not forked with feature
flags.

**Rationale**: The protocol changes are deep enough (different contract, different proof model,
removed features) that a clean separation avoids conditional compilation complexity and makes
each crate self-contained.

### What to Keep From Shasta
- Multicall batching logic (user ops + propose + l1 calls)
- Bridge handler RPC (port 4545)
- Blob encoding (manifest compression via `taiko_protocol::shasta`)
- L2 anchor construction (`anchorV4WithSignalSlots` — unchanged)
- Slot clock, heartbeat, operator management
- Signal slot handling
- Metrics, watchdog, cancellation

### What to Remove
- Forced inclusion subsystem (`forced_inclusion/` module)
- Bond management (`getBond`, `deposit`, `withdraw`)
- Proposal ID tracking (sequential IDs → hash-based)
- `CoreState` queries (`getCoreState`, `getInboxState`, `nextProposalId`)
- Ring buffer queries (`getProposalHash`)
- Proving window / liveness checks
- `activationTimestamp` warmup (replace with `getLastProposalHash` check)
- Verifier / handover window logic (no batched proving needed)
- `proposerChecker` / whitelist checks (anyone can propose)

### What to Add
- Raiko HTTP client for proof generation
- Polling loop for proof readiness
- Proposal hash computation (local, for `parentProposalHash` tracking)
- `maxAnchorBlockNumber` / `maxAnchorBlockHash` fields

---

## Step 1 — Scaffold the `realtime` Crate

### Directory Structure

```
realtime/
├── Cargo.toml
├── src/
│   ├── lib.rs                          # create_realtime_node()
│   ├── raiko/
│   │   └── mod.rs                      # Raiko HTTP client
│   ├── l1/
│   │   ├── mod.rs
│   │   ├── bindings.rs                 # RealTimeInbox + Multicall bindings
│   │   ├── config.rs                   # Contract addresses
│   │   ├── execution_layer.rs          # L1 interaction (slimmed)
│   │   ├── proposal_tx_builder.rs      # Build propose tx with ZK proof
│   │   ├── protocol_config.rs          # 3-field config from RealTimeInbox
│   │   └── abi/
│   │       ├── RealTimeInbox.json      # From realtime/RealtimeInbox.json
│   │       └── Multicall.json          # Copied from shasta
│   ├── l2/
│   │   ├── mod.rs
│   │   ├── bindings.rs                 # Anchor bindings (new ABI)
│   │   ├── execution_layer.rs          # Mostly unchanged
│   │   ├── extra_data.rs               # Copied (or removed if no proposal_id)
│   │   └── abi/
│   │       └── Anchor.json             # From realtime/Anchor.json
│   ├── node/
│   │   ├── mod.rs                      # Simplified main loop
│   │   └── proposal_manager/
│   │       ├── mod.rs                  # Slimmed ProposalManager
│   │       ├── proposal.rs             # New Proposal struct
│   │       ├── batch_builder.rs        # Simplified builder
│   │       ├── l2_block_payload.rs     # Copied
│   │       └── bridge_handler.rs       # Copied
│   ├── shared_abi/
│   │   ├── mod.rs
│   │   ├── bindings.rs
│   │   └── Bridge.json                 # Copied from shasta
│   ├── chain_monitor/
│   │   └── mod.rs                      # Listen for ProposedAndProved events
│   ├── metrics/
│   │   └── mod.rs
│   └── utils/
│       └── config.rs                   # RealtimeConfig
```

### Cargo.toml

```toml
[package]
name = "realtime"
version = "0.1.0"
edition = "2021"

[dependencies]
# Same workspace deps as shasta, plus:
reqwest = { version = "0.12", features = ["json"] }  # For Raiko HTTP client
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# Workspace dependencies (same as shasta)
alloy = { workspace = true }
alloy-json-rpc = { workspace = true }
alloy-rlp = { workspace = true }
anyhow = { workspace = true }
common = { workspace = true }
taiko_alethia_reth = { workspace = true }
taiko_bindings = { workspace = true }
taiko_protocol = { workspace = true }
taiko_rpc = { workspace = true }
pacaya = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
jsonrpsee = { workspace = true }
```

### Actions

1. Create the directory structure above.
2. Copy `Cargo.toml` from shasta, rename package to `realtime`, add `reqwest`.
3. Register `realtime` in the workspace `Cargo.toml`.
4. Copy bridge handler, l2_block_payload, shared_abi verbatim — these are unchanged.

---

## Step 2 — Contract Bindings & ABIs

### 2.1 Move ABIs

```
realtime/RealtimeInbox.json → realtime/src/l1/abi/RealTimeInbox.json
realtime/Anchor.json        → realtime/src/l2/abi/Anchor.json
shasta/src/l1/abi/Multicall.json → realtime/src/l1/abi/Multicall.json (copy)
```

### 2.2 L1 Bindings (`realtime/src/l1/bindings.rs`)

```rust
use alloy::sol;

sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    RealTimeInbox,
    "src/l1/abi/RealTimeInbox.json"
);

sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    #[derive(Debug)]
    Multicall,
    "src/l1/abi/Multicall.json"
);
```

**Key changes vs shasta bindings:**
- `SurgeInbox` → `RealTimeInbox`
- Generated types will include:
  - `RealTimeInbox::proposeCall` (data, checkpoint, proof)
  - `IRealTimeInbox::Config` { proofVerifier, signalService, basefeeSharingPctg }
  - `IRealTimeInbox::ProposeInput` { blobReference, signalSlots, maxAnchorBlockNumber }
  - `ProposedAndProved` event

### 2.3 L2 Bindings (`realtime/src/l2/bindings.rs`)

Copy from shasta but point to the new Anchor ABI:

```rust
sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    Anchor,
    "src/l2/abi/Anchor.json"
);
```

The new Anchor ABI includes `anchorV4WithSignalSlots` (same as shasta) plus the new `anchorV5`.
For the initial migration, continue using `anchorV4WithSignalSlots`.

### 2.4 Type Mapping

| Shasta Type | RealTime Type | Notes |
|---|---|---|
| `IInbox::ProposeInput` { deadline, blobReference, numForcedInclusions } | `IRealTimeInbox::ProposeInput` { blobReference, signalSlots, maxAnchorBlockNumber } | No deadline, no forced inclusions; signal slots and max anchor block are first-class |
| `IInbox::Config` (17 fields) | `IRealTimeInbox::Config` (3 fields) | Only proofVerifier, signalService, basefeeSharingPctg |
| `IInbox::CoreState` | N/A (removed) | Replaced by `getLastProposalHash()` |
| `ICheckpointStore::Checkpoint` | `ICheckpointStore::Checkpoint` | Unchanged |

---

## Step 3 — Configuration & Environment

### 3.1 RealTime Config (`realtime/src/utils/config.rs`)

```rust
use alloy::primitives::Address;

pub struct RealtimeConfig {
    pub realtime_inbox: Address,       // REALTIME_INBOX_ADDRESS
    pub proposer_multicall: Address,   // PROPOSER_MULTICALL_ADDRESS (same)
    pub bridge: Address,               // L1_BRIDGE_ADDRESS (same)
    pub raiko_url: String,             // RAIKO_URL
    pub raiko_api_key: Option<String>, // RAIKO_API_KEY (optional)
    pub proof_type: String,            // RAIKO_PROOF_TYPE (e.g. "sgx", "sp1", "native")
    pub raiko_network: String,         // RAIKO_L2_NETWORK
    pub raiko_l1_network: String,      // RAIKO_L1_NETWORK
}

impl RealtimeConfig {
    pub fn read_env_variables() -> Result<Self, anyhow::Error> {
        Ok(Self {
            realtime_inbox: std::env::var("REALTIME_INBOX_ADDRESS")?.parse()?,
            proposer_multicall: std::env::var("PROPOSER_MULTICALL_ADDRESS")?.parse()?,
            bridge: std::env::var("L1_BRIDGE_ADDRESS")?.parse()?,
            raiko_url: std::env::var("RAIKO_URL")
                .unwrap_or_else(|_| "http://localhost:8080".to_string()),
            raiko_api_key: std::env::var("RAIKO_API_KEY").ok(),
            proof_type: std::env::var("RAIKO_PROOF_TYPE")
                .unwrap_or_else(|_| "sgx".to_string()),
            raiko_network: std::env::var("RAIKO_L2_NETWORK")
                .unwrap_or_else(|_| "taiko_mainnet".to_string()),
            raiko_l1_network: std::env::var("RAIKO_L1_NETWORK")
                .unwrap_or_else(|_| "ethereum".to_string()),
        })
    }
}
```

### 3.2 Contract Addresses (`realtime/src/l1/config.rs`)

```rust
pub struct ContractAddresses {
    pub realtime_inbox: Address,       // Was: shasta_inbox
    pub proposer_multicall: Address,   // Same
    pub bridge: Address,               // Same
    // REMOVED: proposer_checker (anyone can propose in RealTime)
}
```

---

## Step 4 — Protocol Config Adapter

### `realtime/src/l1/protocol_config.rs`

```rust
// RealTimeInbox.getConfig() returns only 3 fields
use crate::l1::bindings::IRealTimeInbox::Config;

#[derive(Clone, Default)]
pub struct ProtocolConfig {
    pub basefee_sharing_pctg: u8,
    pub proof_verifier: Address,
    pub signal_service: Address,
}

impl From<&Config> for ProtocolConfig {
    fn from(config: &Config) -> Self {
        Self {
            basefee_sharing_pctg: config.basefeeSharingPctg,
            proof_verifier: config.proofVerifier,
            signal_service: config.signalService,
        }
    }
}
```

**Removed**: `max_anchor_offset` is no longer read from contract config. Use a constant
or derive from the `blockhash()` 256-block limit.

---

## Step 5 — Proposal Struct Changes

### `realtime/src/node/proposal_manager/proposal.rs`

```rust
use alloy::primitives::{Address, B256, FixedBytes};

#[derive(Default, Clone)]
pub struct Proposal {
    // REMOVED: pub id: u64              — no sequential IDs
    pub l2_blocks: Vec<L2BlockV2>,
    pub total_bytes: u64,
    pub coinbase: Address,

    // CHANGED: anchor → maxAnchor
    pub max_anchor_block_number: u64,     // Was: anchor_block_id
    pub max_anchor_block_hash: B256,      // Was: anchor_block_hash (now read from blockhash())
    // REMOVED: anchor_block_timestamp_sec — not needed
    // REMOVED: anchor_state_root          — not in RealTime proposal

    // REMOVED: num_forced_inclusion       — no forced inclusions

    // Proof fields
    pub checkpoint: Checkpoint,           // Same as shasta
    pub parent_proposal_hash: B256,       // NEW: hash chain tracking

    // Surge POC fields (carried over)
    pub user_ops: Vec<UserOp>,
    pub signal_slots: Vec<FixedBytes<32>>,
    pub l1_calls: Vec<L1Call>,

    // NEW: ZK proof (populated after Raiko call)
    pub zk_proof: Option<Vec<u8>>,
}
```

### Proposal Hash Computation

The proposal hash must be computed locally to track `parentProposalHash`:

```rust
impl Proposal {
    /// Compute the proposalHash as the on-chain contract does:
    /// keccak256(abi.encode(
    ///     parentProposalHash,
    ///     maxAnchorBlockNumber,   // padded to 32 bytes
    ///     maxAnchorBlockHash,
    ///     basefeeSharingPctg,     // padded to 32 bytes
    ///     sources[],              // dynamic array
    ///     signalSlotsHash
    /// ))
    pub fn compute_proposal_hash(&self, basefee_sharing_pctg: u8) -> B256 {
        use alloy::sol_types::SolValue;

        let signal_slots_hash = if self.signal_slots.is_empty() {
            B256::ZERO
        } else {
            alloy::primitives::keccak256(self.signal_slots.abi_encode())
        };

        // Build the sources array (DerivationSource[])
        // ... (from blob sidecar data)

        let encoded = (
            self.parent_proposal_hash,
            alloy::primitives::U256::from(self.max_anchor_block_number),
            self.max_anchor_block_hash,
            alloy::primitives::U256::from(basefee_sharing_pctg),
            // sources encoding...
            signal_slots_hash,
        ).abi_encode();

        alloy::primitives::keccak256(encoded)
    }
}
```

---

## Step 6 — Raiko Proof Client

This is the **biggest new component**. It does not exist in shasta.

### `realtime/src/raiko/mod.rs`

```rust
use anyhow::Error;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{debug, info, warn};

#[derive(Clone)]
pub struct RaikoClient {
    client: Client,
    base_url: String,
    api_key: Option<String>,
    proof_type: String,
    l2_network: String,
    l1_network: String,
    prover_address: String,
    poll_interval: Duration,
    max_retries: u32,
}

#[derive(Serialize)]
pub struct RaikoProofRequest {
    pub l2_block_numbers: Vec<u64>,
    pub proof_type: String,
    pub max_anchor_block_number: u64,
    pub parent_proposal_hash: String,       // "0x..."
    pub basefee_sharing_pctg: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub l1_network: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prover: Option<String>,
    pub signal_slots: Vec<String>,          // "0x..." hex strings
    pub sources: Vec<serde_json::Value>,    // DerivationSource[]
    pub checkpoint: Option<RaikoCheckpoint>,
    pub blob_proof_type: String,
}

#[derive(Serialize, Deserialize)]
pub struct RaikoCheckpoint {
    pub block_number: u64,
    pub block_hash: String,
    pub state_root: String,
}

#[derive(Deserialize)]
pub struct RaikoResponse {
    pub status: String,                     // "ok" or "error"
    #[serde(default)]
    pub proof_type: Option<String>,
    #[serde(default)]
    pub data: Option<RaikoData>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum RaikoData {
    Proof { proof: String },
    Status { status: String },
}

impl RaikoClient {
    pub fn new(config: &RealtimeConfig, prover_address: String) -> Self { /* ... */ }

    /// Request a proof and poll until ready.
    /// Returns the raw proof bytes.
    pub async fn get_proof(&self, request: RaikoProofRequest) -> Result<Vec<u8>, Error> {
        let url = format!("{}/v3/proof/batch/realtime", self.base_url);

        for attempt in 0..self.max_retries {
            let mut req = self.client.post(&url)
                .json(&request);

            if let Some(ref key) = self.api_key {
                req = req.header("X-API-KEY", key);
            }

            let resp = req.send().await?;
            let body: RaikoResponse = resp.json().await?;

            if body.status == "error" {
                return Err(anyhow::anyhow!(
                    "Raiko proof failed: {}",
                    body.message.unwrap_or_default()
                ));
            }

            match body.data {
                Some(RaikoData::Proof { proof }) => {
                    info!("ZK proof received (attempt {})", attempt + 1);
                    // Decode hex proof to bytes
                    let proof_bytes = hex::decode(proof.trim_start_matches("0x"))?;
                    return Ok(proof_bytes);
                }
                Some(RaikoData::Status { ref status }) if status == "ZKAnyNotDrawn" => {
                    warn!("Raiko: ZK prover not drawn for this request");
                    return Err(anyhow::anyhow!("ZK prover not drawn"));
                }
                Some(RaikoData::Status { ref status }) => {
                    debug!("Raiko status: {}, polling... (attempt {})", status, attempt + 1);
                    tokio::time::sleep(self.poll_interval).await;
                }
                None => {
                    return Err(anyhow::anyhow!("Raiko: unexpected empty response"));
                }
            }
        }

        Err(anyhow::anyhow!("Raiko: proof not ready after {} attempts", self.max_retries))
    }
}
```

### Integration Point

The Raiko client is called **after** a batch is finalized (all L2 blocks executed, checkpoint
known) and **before** the L1 transaction is built. This is the critical new step in the pipeline.

---

## Step 7 — Proposal Transaction Builder

### `realtime/src/l1/proposal_tx_builder.rs`

Major changes from shasta's `ProposalTxBuilder`:

1. **Remove `checkpoint_signer`** — no more signed checkpoint proofs.
2. **Remove `build_proof_data()`** — replaced by Raiko ZK proof.
3. **Change `build_propose_call()`** to use `RealTimeInbox.propose()` with 3 params.

```rust
pub struct ProposalTxBuilder {
    provider: DynProvider,
    extra_gas_percentage: u64,
    raiko_client: RaikoClient,
    // REMOVED: checkpoint_signer
}

impl ProposalTxBuilder {
    async fn build_propose_call(
        &self,
        batch: &Proposal,
        inbox_address: Address,
    ) -> Result<(Multicall::Call, BlobTransactionSidecar), Error> {
        // 1. Build blob sidecar (same as shasta)
        let (sidecar, _manifest_data) = self.build_blob_sidecar(batch)?;

        // 2. Build ProposeInput (NEW structure)
        //    RealTimeInbox ProposeInput = { blobReference, signalSlots, maxAnchorBlockNumber }
        let input = IRealTimeInbox::ProposeInput {
            blobReference: BlobReference {
                blobStartIndex: 0,
                numBlobs: sidecar.blobs.len().try_into()?,
                offset: U24::ZERO,
            },
            signalSlots: batch.signal_slots.clone(),
            maxAnchorBlockNumber: U48::from(batch.max_anchor_block_number),
        };

        // 3. Encode the input
        let inbox = RealTimeInbox::new(inbox_address, self.provider.clone());
        let encoded_input = inbox.encodeProposeInput(input).call().await?;

        // 4. Use the ZK proof from Raiko (already obtained)
        let proof = Bytes::from(
            batch.zk_proof.as_ref()
                .ok_or_else(|| anyhow::anyhow!("ZK proof not set on proposal"))?
                .clone()
        );

        // 5. Build the propose call with 3 parameters:
        //    propose(bytes _data, Checkpoint _checkpoint, bytes _proof)
        let call = inbox.propose(
            encoded_input,          // _data = abi.encode(ProposeInput)
            batch.checkpoint.clone(),
            proof,
        );

        Ok((
            Multicall::Call {
                target: inbox_address,
                value: U256::ZERO,
                data: call.calldata().clone(),
            },
            sidecar,
        ))
    }
}
```

### Multicall Composition (unchanged pattern)

The multicall still follows the same pattern:
1. User ops (optional)
2. Propose call (with ZK proof instead of signed checkpoint)
3. L1 calls (optional)

---

## Step 8 — L1 Execution Layer

### `realtime/src/l1/execution_layer.rs`

Key changes from shasta:

```rust
pub struct ExecutionLayer {
    common: ExecutionLayerCommon,
    provider: DynProvider,
    preconfer_address: Address,
    pub transaction_monitor: TransactionMonitor,
    contract_addresses: ContractAddresses,
    // CHANGED: InboxInstance → RealTimeInboxInstance
    realtime_inbox: RealTimeInbox::RealTimeInboxInstance<DynProvider>,
    // REMOVED: checkpoint_signer (no more signed proofs)
}
```

### Removed Methods

- `get_inbox_state()` → removed (no CoreState)
- `get_inbox_next_proposal_id()` → removed (no sequential IDs)
- `get_activation_timestamp()` → removed (RealTimeInbox uses `activate()` differently)
- `get_forced_inclusion_*()` → removed (no forced inclusions)
- `get_preconfer_total_bonds()` → removed (no bonds)
- `is_operator_whitelisted()` → removed (anyone can propose)

### New Methods

```rust
impl ExecutionLayer {
    /// Get the last proposal hash from RealTimeInbox
    pub async fn get_last_proposal_hash(&self) -> Result<B256, Error> {
        let hash = self.realtime_inbox
            .getLastProposalHash()
            .call()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to call getLastProposalHash: {e}"))?;
        Ok(hash)
    }

    /// Fetch the 3-field config from RealTimeInbox
    pub async fn fetch_protocol_config(&self) -> Result<ProtocolConfig, Error> {
        let config = self.realtime_inbox
            .getConfig()
            .call()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to call getConfig: {e}"))?;
        Ok(ProtocolConfig::from(&config.config_))
    }
}
```

### Warmup Changes

Replace the `activationTimestamp` wait loop with `getLastProposalHash` check:

```rust
async fn warmup(&mut self) -> Result<(), Error> {
    // Wait for RealTimeInbox activation (lastProposalHash != 0)
    loop {
        let hash = self.ethereum_l1.execution_layer
            .get_last_proposal_hash().await?;
        if hash != B256::ZERO {
            info!("RealTimeInbox is active, lastProposalHash: {}", hash);
            break;
        }
        warn!("RealTimeInbox not yet activated. Waiting...");
        sleep(Duration::from_secs(12)).await;
    }
    Ok(())
}
```

---

## Step 9 — L2 Anchor Transaction

### No Changes Required

The L2 execution layer already uses `anchorV4WithSignalSlots`, which is the correct anchor
function for the RealTime fork. The Anchor contract ABI from `realtime/Anchor.json` includes
this function.

The only change is the ABI file path in the bindings — point to the new `Anchor.json`.

### Anchor Call (unchanged logic)

```rust
// realtime/src/l2/execution_layer.rs
// Same as shasta/src/l2/execution_layer.rs:105-107
let call_builder = self
    .shasta_anchor
    .anchorV4WithSignalSlots(anchor_block_params.0, anchor_block_params.1);
```

### Note on `anchorV5`

The new Anchor ABI includes `anchorV5` with `ProposalParams` and `BlockParams`. This is for
future use. The initial migration should continue using `anchorV4WithSignalSlots`.

---

## Step 10 — Node Main Loop

### `realtime/src/node/mod.rs`

The main loop is **simplified** because:
- No verifier (no separate proving window to monitor)
- No forced inclusion handling
- No proposer checker / whitelist validation
- No bond management
- Proof is obtained before submission (synchronous from node's perspective)

### Simplified Loop

```rust
async fn main_block_preconfirmation_step(&mut self) -> Result<(), Error> {
    let (l2_slot_info, current_status, pending_tx_list) =
        self.get_slot_info_and_status().await?;

    let transaction_in_progress = self.ethereum_l1.execution_layer
        .is_transaction_in_progress().await?;

    // Preconfirmation phase
    if current_status.is_preconfer() && current_status.is_driver_synced() {
        // Head verification (same as shasta)
        if !self.head_verifier.verify(...).await { /* ... */ }

        let l2_slot_context = L2SlotContext { /* ... */ };

        if self.proposal_manager.should_new_block_be_created(&pending_tx_list, &l2_slot_context) {
            if has_pending_txs || has_pending_user_ops {
                let preconfed_block = self.proposal_manager
                    .preconfirm_block(pending_tx_list, &l2_slot_context).await?;
                self.verify_preconfed_block(preconfed_block).await?;
            }
        }
    }

    // Submission phase — NOW includes proof fetching
    if current_status.is_submitter() && !transaction_in_progress {
        // No verifier check needed — just submit if we have finalized batches
        if self.proposal_manager.has_batches_ready_to_submit() {
            self.proposal_manager.try_submit_oldest_batch().await?;
        }
    }

    // Cleanup (simplified — no verifier to clear)
    if !current_status.is_submitter() && !current_status.is_preconfer() {
        if self.proposal_manager.has_batches() {
            self.proposal_manager.reset_builder().await?;
        }
    }

    Ok(())
}
```

### Removed from Loop
- `check_for_missing_proposed_batches()` — no proposal IDs to compare
- `has_verified_unproposed_batches()` — no external verifier
- `check_and_handle_anchor_offset_for_unsafe_l2_blocks()` — simplified (use 256-block limit)
- `get_next_proposal_id()` — no sequential IDs
- Forced inclusion checks

---

## Step 11 — Batch Manager / Proposal Manager

### Key Change: Proof Fetching Before Submission

The batch submission flow now has an additional step between finalization and L1 submission:

```
finalize_current_batch()
        ↓
fetch_proof_from_raiko()     ← NEW
        ↓
send_batch_to_l1()
```

### `try_submit_oldest_batch()` (modified)

```rust
pub async fn try_submit_oldest_batch(&mut self) -> Result<(), Error> {
    if let Some(batch) = self.proposals_to_send.front_mut() {
        // Step 1: If proof not yet obtained, fetch from Raiko
        if batch.zk_proof.is_none() {
            let l2_block_numbers: Vec<u64> = batch.l2_blocks.iter()
                .map(|b| /* get block number from checkpoint or sequential */)
                .collect();

            let request = RaikoProofRequest {
                l2_block_numbers,
                proof_type: self.raiko_client.proof_type.clone(),
                max_anchor_block_number: batch.max_anchor_block_number,
                parent_proposal_hash: format!("0x{}", hex::encode(batch.parent_proposal_hash)),
                basefee_sharing_pctg: self.protocol_config.basefee_sharing_pctg,
                signal_slots: batch.signal_slots.iter()
                    .map(|s| format!("0x{}", hex::encode(s)))
                    .collect(),
                sources: vec![],  // Build from blob data
                checkpoint: Some(RaikoCheckpoint {
                    block_number: batch.checkpoint.blockNumber.to::<u64>(),
                    block_hash: format!("0x{}", hex::encode(batch.checkpoint.blockHash)),
                    state_root: format!("0x{}", hex::encode(batch.checkpoint.stateRoot)),
                }),
                // ... other fields
            };

            let proof = self.raiko_client.get_proof(request).await?;
            batch.zk_proof = Some(proof);
        }

        // Step 2: Submit to L1 (same as shasta, but with ZK proof)
        self.ethereum_l1.execution_layer
            .send_batch_to_l1(batch.clone(), None, None)
            .await?;

        self.proposals_to_send.pop_front();
    }
    Ok(())
}
```

### Proposal Hash Tracking

Since RealTimeInbox uses `lastProposalHash` instead of sequential IDs, the manager must:

1. **On startup**: Read `getLastProposalHash()` from L1 to initialize `parent_proposal_hash`.
2. **After each submission**: Compute and store the new proposal hash locally.
3. **Use `parent_proposal_hash`** when creating each new proposal.

```rust
pub struct ProposalManager {
    // ...
    parent_proposal_hash: B256,  // Tracks the chain head
}

impl ProposalManager {
    async fn create_new_batch(&mut self) -> Result<(), Error> {
        // Read current L1 block for maxAnchorBlockNumber
        let l1_block = self.ethereum_l1.execution_layer.common()
            .get_latest_block_number().await?;

        // Ensure it's within 256 blocks (blockhash() limit)
        let max_anchor = l1_block.saturating_sub(self.l1_height_lag);

        let max_anchor_hash = self.ethereum_l1.execution_layer.common()
            .get_block_hash_by_number(max_anchor).await?;

        self.batch_builder.create_new_batch(
            max_anchor,
            max_anchor_hash,
            self.parent_proposal_hash,
        );

        Ok(())
    }
}
```

---

## Step 12 — Remove Dead Code

Files/modules from shasta that should NOT be copied to realtime:

| Module | Reason |
|---|---|
| `forced_inclusion/mod.rs` | No forced inclusions in RealTime |
| `node/verifier.rs` | No separate proving window / handover verification |
| `node/l2_height_from_l1.rs` | Based on proposal ID lookups (replaced by hash tracking) |
| `l2/extra_data.rs` | Encodes `proposal_id` into block extra data (no IDs in RealTime) |

Dependencies to remove from trait implementations:

| Trait | Reason |
|---|---|
| `PreconferBondProvider` | No bonds |
| `WhitelistProvider` | No whitelist |

---

## Step 13 — Integration Testing

### 13.1 Unit Tests

1. **Proposal hash computation** — verify local hash matches contract's `hashProposal()`.
2. **Commitment hash computation** — verify local commitment hash matches contract's `hashCommitment()`.
3. **Signal slots hash** — verify `bytes32(0)` for empty, `keccak256(abi.encode(slots))` for non-empty.
4. **ProposeInput encoding** — verify `encodeProposeInput()` output matches expectations.

### 13.2 Integration Tests

1. **Raiko client** — mock server returning Registered → WorkInProgress → proof.
2. **Full pipeline** — local anvil + mock Raiko:
   - Preconfirm block → finalize → fetch proof → submit to RealTimeInbox.
3. **Multicall composition** — user op + propose + l1 call in one tx.
4. **Chain recovery** — restart node, read `getLastProposalHash()`, resume.

### 13.3 E2E Test Script

```bash
# 1. Deploy RealTimeInbox on local anvil
# 2. Activate with genesis hash
# 3. Start Raiko mock (return native proof)
# 4. Start realtime node
# 5. Send L2 transactions
# 6. Verify ProposedAndProved event emitted
# 7. Verify lastProposalHash updated
```

---

## Appendix A — File Mapping (Shasta → RealTime)

| Shasta File | RealTime File | Action |
|---|---|---|
| `lib.rs` | `lib.rs` | Rewrite (simplified init) |
| `node/mod.rs` | `node/mod.rs` | Rewrite (simplified loop) |
| `node/verifier.rs` | — | Delete |
| `node/l2_height_from_l1.rs` | — | Delete |
| `node/proposal_manager/mod.rs` | `node/proposal_manager/mod.rs` | Heavy edit (add Raiko, remove FI) |
| `node/proposal_manager/proposal.rs` | `node/proposal_manager/proposal.rs` | Rewrite (new fields) |
| `node/proposal_manager/batch_builder.rs` | `node/proposal_manager/batch_builder.rs` | Edit (remove FI, ID tracking) |
| `node/proposal_manager/bridge_handler.rs` | `node/proposal_manager/bridge_handler.rs` | Copy verbatim |
| `node/proposal_manager/l2_block_payload.rs` | `node/proposal_manager/l2_block_payload.rs` | Copy (remove proposal_id if needed) |
| `l1/bindings.rs` | `l1/bindings.rs` | Rewrite (RealTimeInbox) |
| `l1/config.rs` | `l1/config.rs` | Edit (remove proposer_checker) |
| `l1/execution_layer.rs` | `l1/execution_layer.rs` | Heavy edit (new methods, remove old) |
| `l1/proposal_tx_builder.rs` | `l1/proposal_tx_builder.rs` | Rewrite (ZK proof, new propose call) |
| `l1/protocol_config.rs` | `l1/protocol_config.rs` | Rewrite (3-field config) |
| `l1/abi/SurgeInbox.json` | `l1/abi/RealTimeInbox.json` | Replace |
| `l1/abi/Multicall.json` | `l1/abi/Multicall.json` | Copy |
| `l2/execution_layer.rs` | `l2/execution_layer.rs` | Copy (minor path changes) |
| `l2/bindings.rs` | `l2/bindings.rs` | Copy (new Anchor ABI path) |
| `l2/extra_data.rs` | — | Delete (or adapt if extra_data still needed) |
| `l2/abi/Anchor.json` | `l2/abi/Anchor.json` | Replace with new ABI |
| `forced_inclusion/mod.rs` | — | Delete |
| `chain_monitor/mod.rs` | `chain_monitor/mod.rs` | Edit (listen for ProposedAndProved) |
| `shared_abi/*` | `shared_abi/*` | Copy verbatim |
| `utils/config.rs` | `utils/config.rs` | Rewrite (new env vars) |
| — | `raiko/mod.rs` | **New** |

---

## Appendix B — Environment Variable Changes

| Variable | Shasta | RealTime | Notes |
|---|---|---|---|
| `SHASTA_INBOX_ADDRESS` | Required | — | Removed |
| `REALTIME_INBOX_ADDRESS` | — | Required | **New** |
| `PROPOSER_MULTICALL_ADDRESS` | Required | Required | Same |
| `L1_BRIDGE_ADDRESS` | Required | Required | Same |
| `RAIKO_URL` | — | Required | **New** — e.g. `http://localhost:8080` |
| `RAIKO_API_KEY` | — | Optional | **New** — for authenticated Raiko |
| `RAIKO_PROOF_TYPE` | — | Optional | **New** — default `sgx` |
| `RAIKO_L2_NETWORK` | — | Optional | **New** — default `taiko_mainnet` |
| `RAIKO_L1_NETWORK` | — | Optional | **New** — default `ethereum` |

---

## Appendix C — Raiko API Quick Reference

### Endpoint

```
POST {RAIKO_URL}/v3/proof/batch/realtime
```

### Request (minimum required fields)

```json
{
  "l2_block_numbers": [100, 101, 102],
  "proof_type": "sgx",
  "max_anchor_block_number": 19500000,
  "parent_proposal_hash": "0x00...00",
  "basefee_sharing_pctg": 0
}
```

### Response States

| Response | Meaning | Action |
|---|---|---|
| `data.proof` present | Proof ready | Use it |
| `data.status: "Registered"` | Queued | Poll (same request) |
| `data.status: "WorkInProgress"` | Generating | Poll (same request) |
| `data.status: "ZKAnyNotDrawn"` | Not selected | Don't retry |
| `status: "error"` | Failed | Check `message` |

### Polling Model

Re-submit the **identical** request body. The server deduplicates by request key.
Recommended interval: 5-30 seconds.

See [FETCH_REAL_TIME_PROOF.md](FETCH_REAL_TIME_PROOF.md) for the full API specification.

---

## Summary Checklist

- [ ] Scaffold `realtime/` crate with Cargo.toml
- [ ] Copy and place ABIs (RealTimeInbox, Anchor, Multicall, Bridge)
- [ ] Create L1/L2 bindings with new ABIs
- [ ] Implement `RealtimeConfig` with new env vars
- [ ] Implement `ProtocolConfig` (3-field)
- [ ] Rewrite `Proposal` struct (hash-based, max anchor, no ID)
- [ ] Implement `RaikoClient` with polling
- [ ] Rewrite `ProposalTxBuilder` (ZK proof, new propose signature)
- [ ] Rewrite `ExecutionLayer` (getLastProposalHash, remove bonds/FI)
- [ ] Copy L2 execution layer (anchor unchanged)
- [ ] Simplify node main loop (remove verifier, FI handling)
- [ ] Modify `ProposalManager` (add Raiko call before submission)
- [ ] Implement proposal hash tracking (parent chain)
- [ ] Remove dead code (forced inclusion, verifier, bonds)
- [ ] Update chain monitor for `ProposedAndProved` events
- [ ] Write unit tests for hash computation
- [ ] Write integration tests with mock Raiko
- [ ] E2E test with local anvil
