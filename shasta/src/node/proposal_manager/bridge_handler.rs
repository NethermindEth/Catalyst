use crate::l2::taiko::Taiko;
use crate::shared_abi::bindings::IBridge::Message;
use crate::{
    l1::execution_layer::{ExecutionLayer, L1BridgeHandlerOps},
    l2::execution_layer::L2BridgeHandlerOps,
};
use alloy::primitives::{Address, Bytes, FixedBytes};
use alloy::signers::Signer;
use anyhow::Result;
use common::{l1::ethereum_l1::EthereumL1, utils::cancellation_token::CancellationToken};
use jsonrpsee::server::{RpcModule, ServerBuilder};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::mpsc::{self, Receiver};
use tracing::{error, info, warn};

// Sequence of calls on L1:
// 1. User op calldata submission to the submitter
// 2. Proposal with signal slot
// 3. L1Call initiated by an L2 contract

// Sequence of calls on l2:
// 1. L2Call initiated by an L1 contract

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum UserOpStatus {
    Pending,
    Processing { tx_hash: FixedBytes<32> },
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
        if let Ok(value) = serde_json::to_vec(status) {
            if let Err(e) = self.db.insert(id.to_be_bytes(), value) {
                error!("Failed to write user op status: {}", e);
            }
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
    // For this POC, this is a signature based proof, but must be a merkle proof in production
    pub signal_slot_proof: Bytes,
}

// Data required to build the L2 call transaction initiated by an L1 contract via the bridge
#[derive(Clone, Debug)]
pub struct L2Call {
    pub message_from_l1: Message,
    pub signal_slot_on_l2: FixedBytes<32>,
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
    // Surge: For signing the L1 call signal slot proofs
    l1_call_proof_signer: alloy::signers::local::PrivateKeySigner,
    status_store: UserOpStatusStore,
}

impl BridgeHandler {
    pub async fn new(
        addr: SocketAddr,
        ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
        taiko: Arc<Taiko>,
        cancellation_token: CancellationToken,
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

            // Set status to Pending
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
            // Surge: Hard coding the private key for the POC
            // (This is the first private key from foundry anvil)
            l1_call_proof_signer: alloy::signers::local::PrivateKeySigner::from_bytes(
                &"0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
                    .parse::<alloy::primitives::FixedBytes<32>>()?,
            )?,
            status_store,
        })
    }

    pub fn status_store(&self) -> UserOpStatusStore {
        self.status_store.clone()
    }

    // Returns any L2 calls initiated by an L1 contract via the Bridge.
    // For seamless composability, the users will be submitting a `UserOp` on L1 to interact with
    // the Bridge and any other intermediate contract.
    pub async fn next_user_op_and_l2_call(
        &mut self,
    ) -> Result<Option<(UserOp, L2Call)>, anyhow::Error> {
        if let Ok(user_op) = self.rx.try_recv() {
            // This is the message sent from the L1 contract to the L2, and the
            // associated signal that is set when the user op is executed
            if let Some((message_from_l1, signal_slot_on_l2)) = self
                .ethereum_l1
                .execution_layer
                .find_message_and_signal_slot(user_op.clone())
                .await?
            {
                return Ok(Some((
                    user_op,
                    L2Call {
                        message_from_l1,
                        signal_slot_on_l2,
                    },
                )));
            }

            // No L2 call found in the user op - reject it
            warn!(
                "UserOp id={} rejected: no L2 call found in user op",
                user_op.id
            );
            self.status_store.set(
                user_op.id,
                &UserOpStatus::Rejected {
                    reason: "No L2 call found in user op".to_string(),
                },
            );
        }

        Ok(None)
    }

    // Surge: Finds L1 calls initiated in a specific L2 block
    pub async fn find_l1_call(&mut self, block_id: u64) -> Result<Option<L1Call>, anyhow::Error> {
        if let Some((message_from_l2, signal_slot)) = self
            .taiko
            .l2_execution_layer()
            .find_message_and_signal_slot(block_id)
            .await?
        {
            let signature = self.l1_call_proof_signer.sign_hash(&signal_slot).await?;

            let mut signal_slot_proof = [0_u8; 65];
            signal_slot_proof[..32].copy_from_slice(signature.r().to_be_bytes::<32>().as_slice());
            signal_slot_proof[32..64].copy_from_slice(signature.s().to_be_bytes::<32>().as_slice());
            signal_slot_proof[64] = (signature.v() as u8) + 27;

            return Ok(Some(L1Call {
                message_from_l2,
                signal_slot_proof: Bytes::from(signal_slot_proof),
            }));
        }

        Ok(None)
    }

    pub fn has_pending_user_ops(&self) -> bool {
        !self.rx.is_empty()
    }
}
