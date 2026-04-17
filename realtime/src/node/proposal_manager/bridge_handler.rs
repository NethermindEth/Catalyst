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
#[derive(Clone)]
pub struct UserOpStatusStore {
    db: sled::Db,
}

impl UserOpStatusStore {
    pub fn open(path: &str) -> Result<Self, anyhow::Error> {
        let db = sled::open(path)
            .map_err(|e| anyhow::anyhow!("Failed to open user op status store: {}", e))?;
        Ok(Self { db })
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
                    // Look up L2 transaction by hash
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

    pub async fn find_l1_call(
        &mut self,
        block_id: u64,
        state_root: B256,
    ) -> Result<Option<L1Call>, anyhow::Error> {
        let l2_el = self.taiko.l2_execution_layer();

        if let Some((message_from_l2, signal_slot)) =
            l2_el.find_message_and_signal_slot(block_id).await?
        {
            let signal_slot_proof = l2_el
                .get_hop_proof(signal_slot, block_id, state_root)
                .await?;

            // Simulate the L1 callback (Bridge.processMessage) to detect any
            // L1→L2 return signal the callback will produce. If found, it
            // must be pre-injected into the L2 block's anchor fast signals
            // and committed as a requiredReturnSignal in the inbox proposal.
            //
            // The simulator uses state_override on L1 SignalService so the
            // signal-verification step passes even before the real checkpoint
            // is committed. A None result means the callback does not produce
            // an outbound — classic L1→L2→L1 flow, no deferred-finalize needed.
            let l1_el = &self.ethereum_l1.execution_layer;
            let contracts = l1_el.contract_addresses();
            // L2 bridge address is auto-derived from L2 chain id on the L2
            // side — pull it from there rather than duplicating in config.
            let l2_bridge_address = *l2_el.bridge.address();
            let required_return_signal = match l1_el
                .simulate_l1_callback_return_signal(
                    message_from_l2.clone(),
                    signal_slot_proof.clone(),
                    contracts.bridge,
                    l2_bridge_address,
                )
                .await
            {
                Ok(Some((_return_msg, slot))) => {
                    info!(
                        "L1 callback simulation found return signal slot={} — will use deferred finalize",
                        slot
                    );
                    Some(slot)
                }
                Ok(None) => None,
                Err(e) => {
                    // Simulation failure is not fatal: fall back to classic flow.
                    warn!(
                        "L1 callback simulation failed ({}) — falling back to classic propose",
                        e
                    );
                    None
                }
            };

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
