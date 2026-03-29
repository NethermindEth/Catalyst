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
use tracing::{error, info, warn};

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
    #[serde(default, rename = "chainId")]
    pub chain_id: u64,
}

// Data required to build the L1 call transaction initiated by an L2 contract via the bridge
#[derive(Clone, Debug)]
pub struct L1Call {
    pub message_from_l2: Message,
    pub signal_slot_proof: Bytes,
}

// Data required to build the L2 call transaction initiated by an L1 contract via the bridge
#[derive(Clone, Debug)]
pub struct L2Call {
    pub message_from_l1: Message,
    pub signal_slot_on_l2: FixedBytes<32>,
}

/// Result of routing a UserOp: either it targets L1 (and triggers an L2 bridge call)
/// or it targets L2 (for direct execution on L2, e.g. bridge-out).
pub enum UserOpRouting {
    /// L1 UserOp that triggers a bridge deposit (L1→L2).
    L1ToL2 { user_op: UserOp, l2_call: L2Call },
    /// L2 UserOp for direct execution on L2 (e.g. bridge-out L2→L1).
    L2Direct { user_op: UserOp },
}

#[derive(Clone)]
struct BridgeRpcContext {
    tx: mpsc::Sender<UserOp>,
    status_store: UserOpStatusStore,
    next_id: Arc<AtomicU64>,
}

pub struct BridgeHandler {
    ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
    taiko: Arc<Taiko>,
    rx: Receiver<UserOp>,
    status_store: UserOpStatusStore,
    l1_chain_id: u64,
    l2_chain_id: u64,
}

impl BridgeHandler {
    pub async fn new(
        addr: SocketAddr,
        ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
        taiko: Arc<Taiko>,
        cancellation_token: CancellationToken,
        l1_chain_id: u64,
        l2_chain_id: u64,
    ) -> Result<Self, anyhow::Error> {
        let (tx, rx) = mpsc::channel::<UserOp>(1024);
        let status_store = UserOpStatusStore::open("data/user_op_status")?;

        let rpc_context = BridgeRpcContext {
            tx,
            status_store: status_store.clone(),
            next_id: Arc::new(AtomicU64::new(1)),
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
            l2_chain_id,
        })
    }

    pub fn status_store(&self) -> UserOpStatusStore {
        self.status_store.clone()
    }

    /// Dequeue the next UserOp and route it based on the `chainId` param.
    ///
    /// If `chainId` matches L1, simulates on L1 to extract bridge message (L1→L2 deposit).
    /// If `chainId` matches L2, returns it for direct L2 block inclusion (bridge-out).
    /// If `chainId` is 0 or missing, defaults to L1 (backwards compatible).
    pub async fn next_user_op_routed(&mut self) -> Result<Option<UserOpRouting>, anyhow::Error> {
        let Ok(user_op) = self.rx.try_recv() else {
            return Ok(None);
        };

        if user_op.chain_id == self.l2_chain_id {
            info!(
                "UserOp id={} targets L2 (chainId={}), queueing for L2 execution",
                user_op.id, user_op.chain_id
            );
            return Ok(Some(UserOpRouting::L2Direct { user_op }));
        }

        // Reject unknown chain IDs (0 is allowed as default-to-L1)
        if user_op.chain_id != 0 && user_op.chain_id != self.l1_chain_id {
            warn!(
                "UserOp id={} has unknown chainId={}, rejecting",
                user_op.id, user_op.chain_id
            );
            self.status_store.set(
                user_op.id,
                &UserOpStatus::Rejected {
                    reason: format!("Unknown chainId: {}", user_op.chain_id),
                },
            );
            return Ok(None);
        }

        // L1 UserOp — simulate on L1 to extract bridge message
        if let Some((message_from_l1, signal_slot_on_l2)) = self
            .ethereum_l1
            .execution_layer
            .find_message_and_signal_slot(user_op.clone())
            .await?
        {
            return Ok(Some(UserOpRouting::L1ToL2 {
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

            return Ok(Some(L1Call {
                message_from_l2,
                signal_slot_proof,
            }));
        }

        Ok(None)
    }

    pub fn has_pending_user_ops(&self) -> bool {
        !self.rx.is_empty()
    }
}
