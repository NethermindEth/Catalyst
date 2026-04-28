use crate::l2::taiko::Taiko;
use crate::shared_abi::bindings::IBridge::Message;
use crate::{
    l1::execution_layer::{ExecutionLayer, L1BridgeHandlerOps},
    l2::execution_layer::L2BridgeHandlerOps,
};
use alloy::primitives::{Address, B256, Bytes, FixedBytes};
use anyhow::Result;
use common::{l1::ethereum_l1::EthereumL1, utils::cancellation_token::CancellationToken};
use jsonrpsee::server::{RpcModule, ServerBuilder};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::mpsc::{self, Receiver};
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum UserOpStatus {
    Pending,
    Processing { tx_hash: FixedBytes<32> },
    ProvingBlock { block_id: u64 },
    Rejected { reason: String },
    Executed,
}

/// Disk-backed user op status store using sled.
///
/// Two keyspaces live in this store:
/// - default tree: keyed by `u64` UserOp id (L1→L2→L1 path).
/// - `by_hash` tree: keyed by L2 tx hash `B256` (L2→L1→L2 mempool-picked txs).
#[derive(Clone)]
pub struct UserOpStatusStore {
    db: sled::Db,
    by_hash: sled::Tree,
}

impl UserOpStatusStore {
    pub fn open(path: &str) -> Result<Self, anyhow::Error> {
        let db = sled::open(path)
            .map_err(|e| anyhow::anyhow!("Failed to open user op status store: {}", e))?;
        let by_hash = db
            .open_tree("by_hash")
            .map_err(|e| anyhow::anyhow!("Failed to open by_hash tree: {}", e))?;
        Ok(Self { db, by_hash })
    }

    pub fn set(&self, id: u64, status: &UserOpStatus) {
        if let Ok(value) = serde_json::to_vec(status)
            && let Err(e) = self.db.insert(id.to_be_bytes(), value)
        {
            error!("Failed to write user op status: {}", e);
        }
    }

    pub fn get(&self, id: u64) -> Option<UserOpStatus> {
        self.db
            .get(id.to_be_bytes())
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_slice(&v).ok())
    }

    pub fn remove(&self, id: u64) {
        let _ = self.db.remove(id.to_be_bytes());
    }

    pub fn set_by_hash(&self, hash: B256, status: &UserOpStatus) {
        if let Ok(value) = serde_json::to_vec(status)
            && let Err(e) = self.by_hash.insert(hash.as_slice(), value)
        {
            error!("Failed to write tx status by hash: {}", e);
        }
    }

    pub fn get_by_hash(&self, hash: B256) -> Option<UserOpStatus> {
        self.by_hash
            .get(hash.as_slice())
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_slice(&v).ok())
    }

    pub fn remove_by_hash(&self, hash: B256) {
        let _ = self.by_hash.remove(hash.as_slice());
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct UserOp {
    #[serde(default)]
    pub id: u64,
    pub submitter: Address,
    pub calldata: Bytes,
}

// Data required to build the L1 call transaction initiated by an L2 contract via the bridge
#[derive(Clone, Debug)]
pub struct L1Call {
    pub message_from_l2: Message,
    pub signal_slot_proof: Bytes,
    /// Optional: if the L1 callback triggered by `processMessage` produces an
    /// L1→L2 return signal that the same L2 block consumes as a fast signal,
    /// this is that signal slot. When present, the inbox must defer finalization
    /// of the proposal until this slot is populated on L1 — triggering the
    /// tentativePropose + finalizePropose multicall shape.
    pub required_return_signal: Option<FixedBytes<32>>,
}

// Data required to build the L2 call transaction initiated by an L1 contract via the bridge
#[derive(Clone, Debug)]
pub struct L2Call {
    pub message_from_l1: Message,
    pub signal_slot_on_l2: FixedBytes<32>,
}

/// Routed L1→L2 UserOp: triggers an L2 bridge call via processMessage.
pub struct RoutedUserOp {
    pub user_op: UserOp,
    pub l2_call: L2Call,
}

#[derive(Debug, Deserialize)]
struct TxStatusRequest {
    #[serde(default, rename = "userOpId")]
    user_op_id: Option<u64>,
    #[serde(default, rename = "txHash")]
    tx_hash: Option<B256>,
}

#[derive(Clone)]
struct BridgeRpcContext {
    tx: mpsc::Sender<UserOp>,
    status_store: UserOpStatusStore,
    next_id: Arc<AtomicU64>,
    ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
    taiko: Arc<Taiko>,
    last_finalized_block_number: Arc<AtomicU64>,
}

pub struct BridgeHandler {
    ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
    taiko: Arc<Taiko>,
    rx: Receiver<UserOp>,
    status_store: UserOpStatusStore,
    #[allow(dead_code)]
    l1_chain_id: u64,
}

impl BridgeHandler {
    pub async fn new(
        addr: SocketAddr,
        ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
        taiko: Arc<Taiko>,
        cancellation_token: CancellationToken,
        l1_chain_id: u64,
        last_finalized_block_number: Arc<AtomicU64>,
    ) -> Result<Self, anyhow::Error> {
        let (tx, rx) = mpsc::channel::<UserOp>(1024);
        let status_store = UserOpStatusStore::open("data/user_op_status")?;

        let rpc_context = BridgeRpcContext {
            tx,
            status_store: status_store.clone(),
            next_id: Arc::new(AtomicU64::new(1)),
            ethereum_l1: ethereum_l1.clone(),
            taiko: taiko.clone(),
            last_finalized_block_number,
        };

        let server = ServerBuilder::default()
            .build(addr)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to build RPC server: {}", e))?;

        let mut module = RpcModule::new(rpc_context);

        module.register_async_method("surge_sendUserOp", |params, ctx, _| async move {
            let mut user_op: UserOp = params.parse()?;
            let id = ctx.next_id.fetch_add(1, Ordering::Relaxed);
            user_op.id = id;

            info!(
                "Received UserOp: id={}, submitter={:?}, calldata_len={}",
                id,
                user_op.submitter,
                user_op.calldata.len()
            );

            ctx.status_store.set(id, &UserOpStatus::Pending);

            ctx.tx.send(user_op).await.map_err(|e| {
                error!("Failed to send UserOp to queue: {}", e);
                ctx.status_store.remove(id);
                jsonrpsee::types::ErrorObjectOwned::owned(
                    -32000,
                    "Failed to queue user operation",
                    Some(format!("{}", e)),
                )
            })?;

            Ok::<u64, jsonrpsee::types::ErrorObjectOwned>(id)
        })?;

        module.register_async_method("surge_userOpStatus", |params, ctx, _| async move {
            let id: u64 = params.one()?;

            match ctx.status_store.get(id) {
                Some(status) => Ok::<serde_json::Value, jsonrpsee::types::ErrorObjectOwned>(
                    serde_json::to_value(status).map_err(|e| {
                        jsonrpsee::types::ErrorObjectOwned::owned(
                            -32603,
                            "Serialization error",
                            Some(format!("{}", e)),
                        )
                    })?,
                ),
                None => Err(jsonrpsee::types::ErrorObjectOwned::owned(
                    -32001,
                    "UserOp not found",
                    Some(format!("No user operation with id {}", id)),
                )),
            }
        })?;

        module.register_async_method("surge_txStatus", |params, ctx, _| async move {
            let request: TxStatusRequest = params.parse()?;

            match (request.user_op_id, request.tx_hash) {
                (Some(id), None) => {
                    // Existing userOpId lookup via status store
                    match ctx.status_store.get(id) {
                        Some(status) => serde_json::to_value(status).map_err(|e| {
                            jsonrpsee::types::ErrorObjectOwned::owned(
                                -32603,
                                "Serialization error",
                                Some(format!("{}", e)),
                            )
                        }),
                        None => Err(jsonrpsee::types::ErrorObjectOwned::owned(
                            -32001,
                            "UserOp not found",
                            Some(format!("No user operation with id {}", id)),
                        )),
                    }
                }
                (None, Some(hash)) => {
                    // Prefer the explicit status store for mempool-picked L2→L1→L2 txs —
                    // it carries the full `sequencing → proving → proposing → complete`
                    // lifecycle that async_submitter writes.
                    if let Some(status) = ctx.status_store.get_by_hash(hash) {
                        return serde_json::to_value(status).map_err(|e| {
                            jsonrpsee::types::ErrorObjectOwned::owned(
                                -32603,
                                "Serialization error",
                                Some(format!("{}", e)),
                            )
                        });
                    }

                    // Fallback: derive from on-chain state (used for L1→L2→L1 UserOp
                    // polling by hash, where no store entry exists).
                    let tx = ctx.taiko.get_transaction_by_hash(hash).await.map_err(|e| {
                        debug!("Transaction {} not found on L2: {}", hash, e);
                        jsonrpsee::types::ErrorObjectOwned::owned(
                            -32001,
                            "Transaction not found",
                            Some(format!("L2 transaction {} not found: {}", hash, e)),
                        )
                    })?;

                    let block_number = tx.block_number.ok_or_else(|| {
                        jsonrpsee::types::ErrorObjectOwned::owned(
                            -32001,
                            "Transaction pending",
                            Some("Transaction has not been included in a block yet".to_string()),
                        )
                    })?;

                    let finalized = ctx.last_finalized_block_number.load(Ordering::Relaxed);

                    let status = if block_number <= finalized {
                        UserOpStatus::Executed
                    } else {
                        UserOpStatus::ProvingBlock {
                            block_id: block_number,
                        }
                    };

                    serde_json::to_value(status).map_err(|e| {
                        jsonrpsee::types::ErrorObjectOwned::owned(
                            -32603,
                            "Serialization error",
                            Some(format!("{}", e)),
                        )
                    })
                }
                _ => Err(jsonrpsee::types::ErrorObjectOwned::owned(
                    -32602,
                    "Invalid params",
                    Some("Provide exactly one of 'userOpId' or 'txHash'".to_string()),
                )),
            }
        })?;

        // surge_simulateReturnMessage: given a raw L2 tx (from, to, data),
        // trace it for an L2→L1 outbound, simulate the L1 callback, and return
        // the IBridge.Message that the L1 callback would produce. Users call this
        // before submitting to the L2 mempool so they can embed the correct
        // returnMessage in their calldata.
        module.register_async_method(
            "surge_simulateReturnMessage",
            |params, ctx, _| async move {
                use crate::l1::execution_layer::L1BridgeHandlerOps;

                #[derive(serde::Deserialize)]
                struct SimRequest {
                    from: Address,
                    to: Address,
                    data: Bytes,
                    /// ETH value to attach to the traced tx (required for payable
                    /// L2 entry points like swapETHForTokenViaL1).
                    #[serde(default)]
                    value: Option<alloy::primitives::U256>,
                }

                let req: SimRequest = params.one()?;
                info!(
                    "surge_simulateReturnMessage: from={}, to={}, data_len={}, value={:?}",
                    req.from,
                    req.to,
                    req.data.len(),
                    req.value,
                );

                let l2_el = ctx.taiko.l2_execution_layer();

                // Step 1: trace the L2 tx for outbound Bridge.sendMessage
                let outbound = l2_el
                    .trace_tx_for_outbound_message(req.from, req.to, &req.data, req.value)
                    .await
                    .map_err(|e| {
                        jsonrpsee::types::ErrorObjectOwned::owned(
                            -32000,
                            "L2 trace failed",
                            Some(format!("{e}")),
                        )
                    })?
                    .ok_or_else(|| {
                        jsonrpsee::types::ErrorObjectOwned::owned(
                            -32001,
                            "No outbound Bridge.sendMessage found in trace",
                            None::<String>,
                        )
                    })?;

                // Step 2: simulate the L1 callback
                let l1_el = &ctx.ethereum_l1.execution_layer;
                let bridge_addr = l1_el.contract_addresses().bridge;
                let l2_bridge_addr = *l2_el.bridge.address();

                let (return_msg, return_slot) = l1_el
                    .simulate_l1_callback_return_signal(
                        outbound,
                        Bytes::new(),
                        bridge_addr,
                        l2_bridge_addr,
                    )
                    .await
                    .map_err(|e| {
                        jsonrpsee::types::ErrorObjectOwned::owned(
                            -32000,
                            "L1 callback simulation failed",
                            Some(format!("{e}")),
                        )
                    })?
                    .ok_or_else(|| {
                        jsonrpsee::types::ErrorObjectOwned::owned(
                            -32002,
                            "L1 callback produced no return message",
                            None::<String>,
                        )
                    })?;

                // Return the Message struct fields + signal slot as JSON
                Ok::<serde_json::Value, jsonrpsee::types::ErrorObjectOwned>(serde_json::json!({
                    "message": {
                        "id": return_msg.id,
                        "fee": return_msg.fee,
                        "gasLimit": return_msg.gasLimit,
                        "from": format!("{}", return_msg.from),
                        "srcChainId": return_msg.srcChainId,
                        "srcOwner": format!("{}", return_msg.srcOwner),
                        "destChainId": return_msg.destChainId,
                        "destOwner": format!("{}", return_msg.destOwner),
                        "to": format!("{}", return_msg.to),
                        "value": format!("{}", return_msg.value),
                        "data": format!("0x{}", hex::encode(&return_msg.data)),
                    },
                    "signalSlot": format!("{}", return_slot),
                }))
            },
        )?;

        info!("Bridge handler RPC server starting on {}", addr);
        let handle = server.start(module);

        tokio::spawn(async move {
            cancellation_token.cancelled().await;
            info!("Cancellation token triggered, stopping bridge handler RPC server");
            handle.stop().ok();
        });

        Ok(Self {
            ethereum_l1,
            taiko,
            rx,
            status_store,
            l1_chain_id,
        })
    }

    pub fn status_store(&self) -> UserOpStatusStore {
        self.status_store.clone()
    }

    /// Dequeue the next UserOp, simulate on L1 to extract the bridge message
    /// (L1→L2 deposit). UserOps always target L1.
    pub async fn next_user_op(&mut self) -> Result<Option<RoutedUserOp>, anyhow::Error> {
        let Ok(user_op) = self.rx.try_recv() else {
            return Ok(None);
        };

        // L1 UserOp — simulate on L1 to extract bridge message
        if let Some((message_from_l1, signal_slot_on_l2)) = self
            .ethereum_l1
            .execution_layer
            .find_message_and_signal_slot(user_op.clone())
            .await?
        {
            return Ok(Some(RoutedUserOp {
                user_op,
                l2_call: L2Call {
                    message_from_l1,
                    signal_slot_on_l2,
                },
            }));
        }

        warn!(
            "UserOp id={} targets L1 but no bridge message found",
            user_op.id
        );
        self.status_store.set(
            user_op.id,
            &UserOpStatus::Rejected {
                reason: "L1 UserOp with no bridge message".to_string(),
            },
        );
        Ok(None)
    }

    /// Build an L1Call for a Bridge.sendMessage emitted in the just-preconfirmed
    /// L2 block. The mempool scan is the single source of truth for the return
    /// signal: if it found one, its slot was injected into the L2 anchor's fast
    /// signals and must be carried here as the inbox's `requiredReturnSignal`.
    /// We do not re-simulate — any drift between the two simulations would make
    /// the anchor slot disagree with the inbox's verified slot, which reverts
    /// `_verifySignalSlots` (classic) or `finalizePropose` (deferred).
    pub async fn find_l1_call(
        &mut self,
        block_id: u64,
        state_root: B256,
        required_return_signal: Option<FixedBytes<32>>,
    ) -> Result<Option<L1Call>, anyhow::Error> {
        let l2_el = self.taiko.l2_execution_layer();

        // Retry briefly: the L2 RPC may lag indexing the just-preconfirmed
        // block's logs. Without this, `find_message_and_signal_slot` returns
        // None on the hot path and we skip the L1 call — causing classic
        // propose to revert with `SignalSlotNotSent` if the mempool scan
        // already injected a slot into the anchor.
        let mut attempt = 0u32;
        let message_and_slot = loop {
            if let Some(pair) = l2_el.find_message_and_signal_slot(block_id).await? {
                break Some(pair);
            }
            attempt += 1;
            if attempt >= 5 {
                break None;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        };

        if let Some((message_from_l2, signal_slot)) = message_and_slot {
            let signal_slot_proof = l2_el
                .get_hop_proof(signal_slot, block_id, state_root)
                .await?;

            if required_return_signal.is_some() {
                info!(
                    "Adding L1 call with pre-simulated required return signal — will use deferred finalize"
                );
            }

            return Ok(Some(L1Call {
                message_from_l2,
                signal_slot_proof,
                required_return_signal,
            }));
        }

        Ok(None)
    }

    pub fn has_pending_user_ops(&self) -> bool {
        !self.rx.is_empty()
    }
}
