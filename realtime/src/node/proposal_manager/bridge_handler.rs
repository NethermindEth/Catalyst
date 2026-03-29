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

/// Determine the target chain of a UserOp by checking the EIP-712 signature.
///
/// The UserOpsSubmitter uses EIP-712 with domain `(name="UserOpsSubmitter", version="1",
/// chainId, verifyingContract=submitter)`. We decode the `executeBatch(ops[], signature)`
/// calldata, compute the EIP-712 digest for both L1 and L2 chain IDs, and ecrecover.
/// The chain ID that produces a valid recovery (non-zero address) is the target chain.
fn detect_target_chain(user_op: &UserOp, l1_chain_id: u64, l2_chain_id: u64) -> Option<u64> {
    use alloy::sol;
    use alloy::sol_types::{SolCall, SolValue};

    // ABI definition matching UserOpsSubmitter.executeBatch
    sol! {
        struct UserOpSol {
            address target;
            uint256 value;
            bytes data;
        }

        function executeBatch(UserOpSol[] calldata _ops, bytes calldata _signature) external;
    }

    // Decode the calldata
    let decoded = executeBatchCall::abi_decode(&user_op.calldata).ok()?;
    let ops = &decoded._ops;
    let signature = &decoded._signature;

    if signature.len() != 65 {
        warn!("UserOp id={}: signature length {} != 65", user_op.id, signature.len());
        return None;
    }

    // EIP-712 type hashes
    let userop_typehash = alloy::primitives::keccak256(
        b"UserOp(address target,uint256 value,bytes data)",
    );
    let executebatch_typehash = alloy::primitives::keccak256(
        b"ExecuteBatch(UserOp[] ops)UserOp(address target,uint256 value,bytes data)",
    );

    // Hash each op: keccak256(abi.encode(typehash, target, value, keccak256(data)))
    let mut op_hashes = Vec::with_capacity(ops.len());
    for op in ops {
        let data_hash = alloy::primitives::keccak256(&op.data);
        let encoded = (userop_typehash, op.target, op.value, data_hash).abi_encode();
        op_hashes.push(alloy::primitives::keccak256(&encoded));
    }

    // keccak256(abi.encodePacked(opHashes))
    let mut packed = Vec::with_capacity(op_hashes.len() * 32);
    for h in &op_hashes {
        packed.extend_from_slice(h.as_slice());
    }
    let ops_array_hash = alloy::primitives::keccak256(&packed);

    // struct hash = keccak256(abi.encode(EXECUTEBATCH_TYPEHASH, ops_array_hash))
    let struct_hash = alloy::primitives::keccak256(
        &(executebatch_typehash, ops_array_hash).abi_encode(),
    );

    // Parse the 65-byte signature
    let sig = alloy::signers::Signature::try_from(signature.as_ref()).ok()?;

    // Try both chain IDs
    for chain_id in [l1_chain_id, l2_chain_id] {
        let domain_separator = compute_domain_separator(chain_id, user_op.submitter);

        // EIP-712 digest: keccak256("\x19\x01" || domainSeparator || structHash)
        let mut digest_input = Vec::with_capacity(2 + 32 + 32);
        digest_input.extend_from_slice(&[0x19, 0x01]);
        digest_input.extend_from_slice(domain_separator.as_slice());
        digest_input.extend_from_slice(struct_hash.as_slice());
        let digest = alloy::primitives::keccak256(&digest_input);

        if let Ok(recovered) = sig.recover_address_from_prehash(&digest) {
            if recovered != Address::ZERO {
                info!(
                    "UserOp id={}: signature valid for chain_id={} (recovered={})",
                    user_op.id, chain_id, recovered
                );
                return Some(chain_id);
            }
        }
    }

    warn!("UserOp id={}: could not determine target chain", user_op.id);
    None
}

/// Compute EIP-712 domain separator for UserOpsSubmitter(name="UserOpsSubmitter", version="1")
fn compute_domain_separator(chain_id: u64, verifying_contract: Address) -> B256 {
    use alloy::sol_types::SolValue;

    let type_hash = alloy::primitives::keccak256(
        b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
    );
    let name_hash = alloy::primitives::keccak256(b"UserOpsSubmitter");
    let version_hash = alloy::primitives::keccak256(b"1");

    alloy::primitives::keccak256(
        &(type_hash, name_hash, version_hash, alloy::primitives::U256::from(chain_id), verifying_contract).abi_encode(),
    )
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

    /// Dequeue the next UserOp and route it to the correct chain.
    ///
    /// Parses the EIP-712 signature in the `executeBatch` calldata to determine
    /// which chain the UserOp targets. If signed for L1, simulates on L1 to
    /// extract the bridge message. If signed for L2, returns it for direct
    /// L2 block inclusion.
    pub async fn next_user_op_routed(
        &mut self,
    ) -> Result<Option<UserOpRouting>, anyhow::Error> {
        let Ok(user_op) = self.rx.try_recv() else {
            return Ok(None);
        };

        let target_chain = detect_target_chain(&user_op, self.l1_chain_id, self.l2_chain_id);

        match target_chain {
            Some(chain_id) if chain_id == self.l1_chain_id => {
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

                // L1 simulation found no bridge message — still an L1 UserOp (non-bridge)
                warn!(
                    "UserOp id={} targets L1 but no bridge message found, treating as L1 UserOp without bridge",
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
            Some(chain_id) if chain_id == self.l2_chain_id => {
                // L2 UserOp — execute directly on L2
                info!(
                    "UserOp id={} targets L2 (chain_id={}), queueing for L2 execution",
                    user_op.id, chain_id
                );
                Ok(Some(UserOpRouting::L2Direct { user_op }))
            }
            _ => {
                warn!(
                    "UserOp id={} rejected: could not determine target chain from signature",
                    user_op.id
                );
                self.status_store.set(
                    user_op.id,
                    &UserOpStatus::Rejected {
                        reason: "Could not determine target chain from signature".to_string(),
                    },
                );
                Ok(None)
            }
        }
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
