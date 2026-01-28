use crate::l2::taiko::Taiko;
use crate::shared_abi::bindings::IBridge::Message;
use crate::{
    l1::{
        bindings::UserOpsSubmitter::UserOp,
        execution_layer::{ExecutionLayer, L1BridgeHandlerOps},
    },
    l2::execution_layer::L2BridgeHandlerOps,
};
use alloy::primitives::{Address, Bytes, FixedBytes, U256};
use alloy::signers::Signer;
use anyhow::Result;
use common::{l1::ethereum_l1::EthereumL1, utils::cancellation_token::CancellationToken};
use jsonrpsee::server::{RpcModule, ServerBuilder};
use serde::Deserialize;
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::mpsc::{self, Receiver};
use tracing::{error, info};

#[derive(Deserialize)]
pub struct SignedUserOp {
    // `UserOpsSubmitter` contract for the user.
    // Can be replaced with a Safe multisig or equivalent user ops based service contract
    submitter: String,
    target: String,
    value: u64,
    data: Vec<u8>,
    // Signature on the hashed UserOp { target, value, data }
    signature: Vec<u8>,
}

// Sequence of calls on L1:
// 1. User op submission via `UserOpSubmitter`
// 2. Proposal with signal slot
// 3. L1Call initiated by an L2 contract

// Sequence of calls on l2:
// 1. L2Call initiated by an L1 contract

// Data to allow construction of the user op transaction
#[derive(Clone, Debug)]
pub struct UserOpData {
    pub user_op: UserOp,
    pub user_op_signature: Bytes,
    pub user_op_submitter: Address,
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

pub struct BridgeHandler {
    ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
    taiko: Arc<Taiko>,
    rx: Receiver<SignedUserOp>,
    // Surge: For signing the L1 call signal slot proofs
    l1_call_proof_signer: alloy::signers::local::PrivateKeySigner,
}

impl BridgeHandler {
    pub async fn new(
        addr: SocketAddr,
        ethereum_l1: Arc<EthereumL1<ExecutionLayer>>,
        taiko: Arc<Taiko>,
        cancellation_token: CancellationToken,
    ) -> Result<Self, anyhow::Error> {
        let (tx, rx) = mpsc::channel::<SignedUserOp>(1024);

        let server = ServerBuilder::default()
            .build(addr)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to build RPC server: {}", e))?;

        let mut module = RpcModule::new(tx);

        module.register_async_method("surge_sendUserOp", |params, tx_context, _| async move {
            let signed_user_op: SignedUserOp = params.parse()?;

            info!(
                "Received UserOp: target={:?}, value={:?}",
                signed_user_op.target, signed_user_op.value
            );

            tx_context.send(signed_user_op).await.map_err(|e| {
                error!("Failed to send UserOp to queue: {}", e);
                jsonrpsee::types::ErrorObjectOwned::owned(
                    -32000,
                    "Failed to queue user operation",
                    Some(format!("{}", e)),
                )
            })?;

            Ok::<String, jsonrpsee::types::ErrorObjectOwned>(
                "UserOp queued successfully".to_string(),
            )
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
        })
    }

    // Returns any L2 calls initiated by an L1 contract via the Bridge.
    // For seamless composability, the users will be submitting a `UserOp` on L1 to interact with
    // the Bridge and any other intermediate contract.
    pub async fn next_user_op_and_l2_call(
        &mut self,
    ) -> Result<Option<(UserOpData, L2Call)>, anyhow::Error> {
        if let Some(signed_user_op) = self.rx.recv().await {
            let user_op = UserOp {
                target: signed_user_op.target.parse()?,
                value: U256::from(signed_user_op.value),
                data: Bytes::from(signed_user_op.data),
            };

            let user_op_data = UserOpData {
                user_op,
                user_op_submitter: signed_user_op.submitter.parse()?,
                user_op_signature: Bytes::from(signed_user_op.signature),
            };

            // This is the message sent from the L1 contract to the L2, and the
            // associated signal that is set when the user op is executed
            if let Some((message_from_l1, signal_slot_on_l2)) = self
                .ethereum_l1
                .execution_layer
                .find_message_and_signal_slot(user_op_data.clone())
                .await?
            {
                return Ok(Some((
                    user_op_data,
                    L2Call {
                        message_from_l1,
                        signal_slot_on_l2,
                    },
                )));
            }
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
}
