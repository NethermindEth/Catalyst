# Auto-Process L1 Bridge Deposits Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The Catalyst sequencer should automatically detect L1 bridge `MessageSent` events and include `processMessage` transactions in L2 blocks, without requiring users to submit UserOps.

**Architecture:** A new `DepositWatcher` polls L1 for `MessageSent` events on the bridge contract. Discovered deposits are queued as `L2Call` structs into the existing `BridgeHandler`, which already knows how to construct `processMessage` L2 transactions and inject them into block building. The `disable_bridging` gate is removed so the bridge handler starts.

**Tech Stack:** Rust, Alloy (Ethereum RPC), Tokio (async), existing Catalyst crate structure

---

## File Map

| File | Action | Responsibility |
|------|--------|---------------|
| `realtime/src/l1/deposit_watcher.rs` | Create | Poll L1 bridge for MessageSent+SignalSent events, queue L2Calls |
| `realtime/src/l1/mod.rs` | Modify | Add `pub mod deposit_watcher;` |
| `realtime/src/node/proposal_manager/bridge_handler.rs` | Modify | Add deposit queue intake alongside UserOp queue |
| `realtime/src/node/proposal_manager/mod.rs` | Modify | Drain deposit queue in block building |
| `realtime/src/lib.rs` | Modify | Remove `disable_bridging` gate, start watcher |

---

### Task 1: Create the L1 Deposit Watcher

**Files:**
- Create: `realtime/src/l1/deposit_watcher.rs`
- Modify: `realtime/src/l1/mod.rs`

- [ ] **Step 1: Create `deposit_watcher.rs`**

This module polls L1 for `MessageSent` events on the bridge contract and co-located `SignalSent` events on the signal service. It filters for messages targeting our L2 chain ID and sends discovered `(Message, signal_slot)` pairs through a channel.

```rust
// realtime/src/l1/deposit_watcher.rs

use crate::shared_abi::bindings::{
    Bridge::MessageSent,
    IBridge::Message,
    SignalService::SignalSent,
};
use alloy::{
    primitives::{Address, FixedBytes},
    providers::{DynProvider, Provider},
    rpc::types::Filter,
    sol_types::SolEvent,
};
use anyhow::Result;
use common::utils::cancellation_token::CancellationToken;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::node::proposal_manager::bridge_handler::L2Call;

/// Polls L1 for bridge deposit events and queues them for L2 processing.
pub struct DepositWatcher {
    provider: DynProvider,
    bridge_address: Address,
    signal_service_address: Address,
    l2_chain_id: u64,
    tx: mpsc::Sender<L2Call>,
    cancel_token: CancellationToken,
}

impl DepositWatcher {
    pub fn new(
        provider: DynProvider,
        bridge_address: Address,
        signal_service_address: Address,
        l2_chain_id: u64,
        tx: mpsc::Sender<L2Call>,
        cancel_token: CancellationToken,
    ) -> Self {
        Self {
            provider,
            bridge_address,
            signal_service_address,
            l2_chain_id,
            tx,
            cancel_token,
        }
    }

    /// Start polling in a background task. Returns the join handle.
    pub fn start(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            if let Err(e) = self.run().await {
                error!("DepositWatcher exited with error: {}", e);
            }
        })
    }

    async fn run(self) -> Result<()> {
        // Start from the latest block
        let mut from_block = self
            .provider
            .get_block_number()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get block number: {}", e))?;

        info!(
            "DepositWatcher started: bridge={}, signal_service={}, l2_chain_id={}, from_block={}",
            self.bridge_address, self.signal_service_address, self.l2_chain_id, from_block
        );

        loop {
            if self.cancel_token.is_cancelled() {
                info!("DepositWatcher shutting down");
                return Ok(());
            }

            tokio::time::sleep(tokio::time::Duration::from_secs(6)).await;

            let latest_block = match self.provider.get_block_number().await {
                Ok(n) => n,
                Err(e) => {
                    warn!("DepositWatcher: failed to get block number: {}", e);
                    continue;
                }
            };

            if latest_block < from_block {
                continue;
            }

            match self.scan_range(from_block, latest_block).await {
                Ok(count) => {
                    if count > 0 {
                        info!(
                            "DepositWatcher: found {} deposits in blocks {}..{}",
                            count, from_block, latest_block
                        );
                    }
                    from_block = latest_block + 1;
                }
                Err(e) => {
                    warn!(
                        "DepositWatcher: error scanning blocks {}..{}: {}",
                        from_block, latest_block, e
                    );
                    // Retry same range next iteration
                }
            }
        }
    }

    async fn scan_range(&self, from_block: u64, to_block: u64) -> Result<usize> {
        // Query MessageSent events from the bridge
        let bridge_filter = Filter::new()
            .address(self.bridge_address)
            .event_signature(MessageSent::SIGNATURE_HASH)
            .from_block(from_block)
            .to_block(to_block);

        let bridge_logs = self
            .provider
            .get_logs(&bridge_filter)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get MessageSent logs: {}", e))?;

        if bridge_logs.is_empty() {
            return Ok(0);
        }

        // Query SignalSent events from the signal service in the same range
        let signal_filter = Filter::new()
            .address(self.signal_service_address)
            .event_signature(SignalSent::SIGNATURE_HASH)
            .from_block(from_block)
            .to_block(to_block);

        let signal_logs = self
            .provider
            .get_logs(&signal_filter)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get SignalSent logs: {}", e))?;

        // Index signal slots by block number + tx index for matching
        let mut signal_by_tx: std::collections::HashMap<(u64, u64), FixedBytes<32>> =
            std::collections::HashMap::new();

        for log in &signal_logs {
            if let (Some(block_number), Some(tx_index)) =
                (log.block_number, log.transaction_index)
            {
                let log_data = alloy::primitives::LogData::new_unchecked(
                    log.topics().to_vec(),
                    log.data().data.clone(),
                );
                if let Ok(decoded) = SignalSent::decode_log_data(&log_data) {
                    signal_by_tx.insert((block_number, tx_index), decoded.slot);
                }
            }
        }

        let mut count = 0;

        for log in &bridge_logs {
            let log_data = alloy::primitives::LogData::new_unchecked(
                log.topics().to_vec(),
                log.data().data.clone(),
            );

            let decoded = match MessageSent::decode_log_data(&log_data) {
                Ok(d) => d,
                Err(e) => {
                    warn!("Failed to decode MessageSent: {}", e);
                    continue;
                }
            };

            // Only process messages targeting our L2
            if decoded.message.destChainId != self.l2_chain_id {
                debug!(
                    "Skipping message with destChainId={} (want {})",
                    decoded.message.destChainId, self.l2_chain_id
                );
                continue;
            }

            // Find matching signal slot from the same transaction
            let signal_slot = if let (Some(block_number), Some(tx_index)) =
                (log.block_number, log.transaction_index)
            {
                signal_by_tx.get(&(block_number, tx_index)).copied()
            } else {
                None
            };

            let Some(signal_slot) = signal_slot else {
                warn!(
                    "No matching SignalSent for MessageSent in block={:?} tx={:?}",
                    log.block_number, log.transaction_index
                );
                continue;
            };

            let l2_call = L2Call {
                message_from_l1: decoded.message,
                signal_slot_on_l2: signal_slot,
            };

            if let Err(e) = self.tx.send(l2_call).await {
                error!("Failed to queue deposit L2Call: {}", e);
            } else {
                count += 1;
            }
        }

        Ok(count)
    }
}
```

- [ ] **Step 2: Register the module in `l1/mod.rs`**

Add the new module to `realtime/src/l1/mod.rs`:

```rust
pub mod bindings;
pub mod config;
pub mod deposit_watcher;
pub mod execution_layer;
pub mod proposal_tx_builder;
pub mod protocol_config;
```

- [ ] **Step 3: Verify it compiles**

Run from `/tmp/catalyst`:
```bash
cargo check -p realtime 2>&1 | tail -20
```

Expected: may have unused warnings but no errors (the watcher isn't wired in yet).

- [ ] **Step 4: Commit**

```bash
git add realtime/src/l1/deposit_watcher.rs realtime/src/l1/mod.rs
git commit -m "feat(realtime): add L1 deposit watcher for bridge MessageSent events"
```

---

### Task 2: Add deposit queue to BridgeHandler

**Files:**
- Modify: `realtime/src/node/proposal_manager/bridge_handler.rs`

The `BridgeHandler` currently only receives bridge data from UserOps via the RPC channel. We add a second channel for direct L1 deposits discovered by the watcher.

- [ ] **Step 1: Add deposit receiver field and constructor parameter**

In `realtime/src/node/proposal_manager/bridge_handler.rs`, add the deposit channel:

Add a new field to `BridgeHandler`:

```rust
pub struct BridgeHandler {
    ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
    taiko: Arc<Taiko>,
    rx: Receiver<UserOp>,
    deposit_rx: Receiver<L2Call>,
    status_store: UserOpStatusStore,
}
```

Update `BridgeHandler::new()` to accept and store the receiver:

```rust
    pub async fn new(
        addr: SocketAddr,
        ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
        taiko: Arc<Taiko>,
        cancellation_token: CancellationToken,
        deposit_rx: Receiver<L2Call>,
    ) -> Result<Self, anyhow::Error> {
```

And in the return value:

```rust
        Ok(Self {
            ethereum_l1,
            taiko,
            rx,
            deposit_rx,
            status_store,
        })
```

- [ ] **Step 2: Add `next_deposit_l2_call()` method**

Add a method that drains the deposit queue, returning the next L2Call from direct deposits:

```rust
    pub fn next_deposit_l2_call(&mut self) -> Option<L2Call> {
        self.deposit_rx.try_recv().ok()
    }

    pub fn has_pending_deposits(&self) -> bool {
        !self.deposit_rx.is_empty()
    }
```

- [ ] **Step 3: Verify it compiles**

```bash
cargo check -p realtime 2>&1 | tail -20
```

Expected: errors in `mod.rs` where `BridgeHandler::new()` is called without the new parameter. We fix that in Task 3.

- [ ] **Step 4: Commit**

```bash
git add realtime/src/node/proposal_manager/bridge_handler.rs
git commit -m "feat(realtime): add deposit receiver channel to BridgeHandler"
```

---

### Task 3: Wire deposit watcher into node startup and block building

**Files:**
- Modify: `realtime/src/node/proposal_manager/mod.rs`
- Modify: `realtime/src/lib.rs`

- [ ] **Step 1: Create deposit channel and pass to BridgeHandler in `mod.rs`**

In `realtime/src/node/proposal_manager/mod.rs`, update `BatchManager::new()` to create the deposit channel and return the sender:

Add `use tokio::sync::mpsc;` at the top (already imported for other uses).

Change the return type and body of `BatchManager::new()`:

```rust
    pub async fn new(
        l1_height_lag: u64,
        config: BatchBuilderConfig,
        ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
        taiko: Arc<Taiko>,
        metrics: Arc<Metrics>,
        cancel_token: CancellationToken,
        last_finalized_block_hash: B256,
        raiko_client: RaikoClient,
        basefee_sharing_pctg: u8,
        proof_request_bypass: bool,
    ) -> Result<(Self, mpsc::Sender<bridge_handler::L2Call>), Error> {
        // ... existing code ...

        let (deposit_tx, deposit_rx) = mpsc::channel::<bridge_handler::L2Call>(256);

        let bridge_addr: SocketAddr = "0.0.0.0:4545".parse()?;
        let bridge_handler = Arc::new(Mutex::new(
            BridgeHandler::new(
                bridge_addr,
                ethereum_l1.clone(),
                taiko.clone(),
                cancel_token.clone(),
                deposit_rx,
            )
            .await?,
        ));

        // ... rest unchanged ...

        Ok((Self {
            batch_builder: BatchBuilder::new(
                config,
                ethereum_l1.slot_clock.clone(),
                metrics.clone(),
            ),
            async_submitter,
            bridge_handler,
            ethereum_l1,
            taiko,
            l1_height_lag,
            metrics,
            cancel_token,
            last_finalized_block_hash,
        }, deposit_tx))
    }
```

- [ ] **Step 2: Add deposit consumption to block building**

In the same file, update `add_pending_l2_call_to_draft_block()` to also check deposits when no UserOp is pending:

```rust
    async fn add_pending_l2_call_to_draft_block(
        &mut self,
        l2_draft_block: &mut L2BlockV2Draft,
    ) -> Result<Option<(Option<UserOp>, FixedBytes<32>)>, anyhow::Error> {
        // First, try UserOp-triggered L2 calls (existing behavior)
        if let Some((user_op_data, l2_call)) = self
            .bridge_handler
            .lock()
            .await
            .next_user_op_and_l2_call()
            .await?
        {
            info!("Processing pending L2 call from UserOp: {:?}", l2_call);

            let l2_call_bridge_tx = self
                .taiko
                .l2_execution_layer()
                .construct_l2_call_tx(l2_call.message_from_l1)
                .await?;

            info!(
                "Inserting L2 call bridge transaction into tx list: {:?}",
                l2_call_bridge_tx
            );

            l2_draft_block
                .prebuilt_tx_list
                .tx_list
                .push(l2_call_bridge_tx);

            return Ok(Some((Some(user_op_data), l2_call.signal_slot_on_l2)));
        }

        // Then, try direct deposit L2 calls from the watcher
        if let Some(l2_call) = self.bridge_handler.lock().await.next_deposit_l2_call() {
            info!(
                "Processing pending L2 call from direct deposit: destOwner={}, value={}",
                l2_call.message_from_l1.destOwner, l2_call.message_from_l1.value
            );

            let l2_call_bridge_tx = self
                .taiko
                .l2_execution_layer()
                .construct_l2_call_tx(l2_call.message_from_l1)
                .await?;

            l2_draft_block
                .prebuilt_tx_list
                .tx_list
                .push(l2_call_bridge_tx);

            return Ok(Some((None, l2_call.signal_slot_on_l2)));
        }

        Ok(None)
    }
```

Update `add_draft_block_to_proposal()` to handle the `Option<UserOp>`:

```rust
    async fn add_draft_block_to_proposal(
        &mut self,
        mut l2_draft_block: L2BlockV2Draft,
        l2_slot_context: &L2SlotContext,
        operation_type: OperationType,
    ) -> Result<BuildPreconfBlockResponse, Error> {
        let mut anchor_signal_slots: Vec<FixedBytes<32>> = vec![];

        debug!("Checking for pending L2 calls");
        if let Some((maybe_user_op, signal_slot)) = self
            .add_pending_l2_call_to_draft_block(&mut l2_draft_block)
            .await?
        {
            if let Some(user_op_data) = maybe_user_op {
                self.batch_builder.add_user_op(user_op_data)?;
            }
            self.batch_builder.add_signal_slot(signal_slot)?;
            anchor_signal_slots.push(signal_slot);
        } else {
            debug!("No pending L2 calls");
        }
        // ... rest unchanged ...
    }
```

Also update `has_pending_user_ops()` to include deposits:

```rust
    pub async fn has_pending_user_ops(&self) -> bool {
        let handler = self.bridge_handler.lock().await;
        handler.has_pending_user_ops() || handler.has_pending_deposits()
    }
```

- [ ] **Step 3: Update `lib.rs` — remove gate, start watcher, fix `BatchManager::new()` call**

In `realtime/src/lib.rs`:

Remove the `disable_bridging` gate (lines 35-39):

```rust
    // DELETE these lines:
    // if !config.disable_bridging {
    //     return Err(anyhow::anyhow!(
    //         "Bridging is not implemented. Exiting RealTime node creation."
    //     ));
    // }
```

Update the `BatchManager::new()` call site in `Node::new()`. Since `BatchManager::new()` now returns a tuple, update `node/mod.rs` `Node::new()`:

In `realtime/src/node/mod.rs`, change:

```rust
        let proposal_manager = BatchManager::new(
            // ... args ...
        )
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create BatchManager: {}", e))?;
```

To:

```rust
        let (proposal_manager, deposit_tx) = BatchManager::new(
            // ... args ...
        )
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create BatchManager: {}", e))?;
```

Then return `deposit_tx` from `Node::new()` by adding it to the struct or passing it back. The simplest approach: store it in `Node` temporarily and start the watcher in `entrypoint()`.

Add `deposit_tx` field to `Node`:

```rust
pub struct Node {
    // ... existing fields ...
    deposit_tx: Option<mpsc::Sender<bridge_handler::L2Call>>,
}
```

Set it in `Node::new()`:

```rust
        Ok(Self {
            // ... existing fields ...
            deposit_tx: Some(deposit_tx),
        })
```

Then in `lib.rs`, after creating the node but before calling `entrypoint()`, start the watcher. Actually, it's cleaner to start it inside `entrypoint()`. Update `entrypoint()` in `node/mod.rs`:

```rust
    pub async fn entrypoint(mut self) -> Result<(), Error> {
        info!("Starting RealTime node");

        if let Err(err) = self.warmup().await {
            error!("Failed to warm up node: {}. Shutting down.", err);
            self.cancel_token.cancel_on_critical_error();
            return Err(anyhow::anyhow!(err));
        }

        info!("Node warmup successful");

        // Start the L1 deposit watcher if we have the channel
        if let Some(deposit_tx) = self.deposit_tx.take() {
            let l1_provider = self.ethereum_l1.execution_layer.common().provider().clone();
            let bridge_address = self.ethereum_l1.execution_layer.contract_addresses().bridge;
            let signal_service = self.ethereum_l1.execution_layer.protocol_config().signal_service;
            let l2_chain_id = self.taiko.l2_execution_layer().chain_id;

            let watcher = crate::l1::deposit_watcher::DepositWatcher::new(
                l1_provider,
                bridge_address,
                signal_service,
                l2_chain_id,
                deposit_tx,
                self.cancel_token.clone(),
            );
            watcher.start();
            info!("L1 deposit watcher started");
        }

        tokio::spawn(async move {
            self.preconfirmation_loop().await;
        });

        Ok(())
    }
```

- [ ] **Step 4: Expose needed fields from ExecutionLayer**

In `realtime/src/l1/execution_layer.rs`, add accessor methods:

```rust
impl ExecutionLayer {
    pub fn contract_addresses(&self) -> &ContractAddresses {
        &self.contract_addresses
    }

    pub fn protocol_config_ref(&self) -> &ProtocolConfig {
        // We need to store the protocol config. Add a field or fetch it.
        // Simplest: store it during construction.
    }
}
```

Actually, the protocol config is fetched after `ExecutionLayer` is created (in `lib.rs:66`). The simplest approach: pass the signal service address through to `Node`. Add it as a parameter to `Node::new()`:

In `lib.rs`, after fetching `protocol_config`:

```rust
    let signal_service_address = protocol_config.signal_service;
```

Pass it to `Node::new()` and store it. Then use it in `entrypoint()`.

Alternatively, expose `contract_addresses` from ExecutionLayer (it's already a field, just needs a pub getter) and store the protocol config's signal_service in it or in `Node`.

The cleanest approach: add `signal_service` to `ContractAddresses`:

In `realtime/src/l1/config.rs`:

```rust
#[derive(Clone)]
pub struct ContractAddresses {
    pub realtime_inbox: Address,
    pub proposer_multicall: Address,
    pub bridge: Address,
    pub signal_service: Address,
}
```

Set it in `ExecutionLayer::new()` after fetching the config:

```rust
        let contract_addresses = ContractAddresses {
            realtime_inbox: specific_config.realtime_inbox,
            proposer_multicall: specific_config.proposer_multicall,
            bridge: specific_config.bridge,
            signal_service: config.signalService,
        };
```

And add the accessor:

```rust
impl ExecutionLayer {
    pub fn contract_addresses(&self) -> &ContractAddresses {
        &self.contract_addresses
    }
}
```

Then in `Node::entrypoint()`:

```rust
        let bridge_address = self.ethereum_l1.execution_layer.contract_addresses().bridge;
        let signal_service = self.ethereum_l1.execution_layer.contract_addresses().signal_service;
```

For the L1 provider, `ExecutionLayerCommon` has a provider. Add a public accessor:

In `realtime/src/l1/execution_layer.rs`:

```rust
impl ExecutionLayer {
    pub fn provider(&self) -> &DynProvider {
        &self.provider
    }
}
```

- [ ] **Step 5: Add needed imports to `node/mod.rs`**

```rust
use crate::node::proposal_manager::bridge_handler;
use tokio::sync::mpsc;
```

- [ ] **Step 6: Verify it compiles**

```bash
cargo check -p realtime 2>&1 | tail -30
```

Expected: PASS (no errors).

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(realtime): wire deposit watcher into block building pipeline

- Remove disable_bridging gate
- Start DepositWatcher in Node::entrypoint()
- BridgeHandler consumes deposits from both UserOp RPC and direct L1 events
- Direct deposits don't require a UserOp, just signal slot + message"
```

---

### Task 4: Verify end-to-end (manual)

- [ ] **Step 1: Build the full project**

```bash
cargo build -p realtime 2>&1 | tail -20
```

Expected: successful build.

- [ ] **Step 2: Run clippy**

```bash
cargo clippy -p realtime -- -D warnings 2>&1 | tail -30
```

Fix any warnings.

- [ ] **Step 3: Run existing tests**

```bash
cargo test -p realtime 2>&1 | tail -20
```

- [ ] **Step 4: Commit any fixes**

```bash
git add -A
git commit -m "fix(realtime): address clippy warnings in deposit watcher"
```
