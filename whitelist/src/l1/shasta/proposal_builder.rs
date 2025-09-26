use crate::l1::shasta::bindings::iinbox;

use super::bindings::lib_manifest;
use alloy::{
    consensus::Transaction,
    eips::{Typed2718, eip2930::AccessList},
    network::{TransactionBuilder, TransactionBuilder4844},
    primitives::{Address, Bytes, Uint},
    rlp::Encodable,
    rpc::types::TransactionRequest,
    sol_types::SolValue,
};
use anyhow::Error;
use common::shared::l2_block::L2Block;

pub async fn build_proposal_tx(
    // &self,
    l2_blocks: Vec<L2Block>,
    last_anchor_origin_height: u64,
    coinbase: Address,
    current_l1_slot_timestamp: u64,
    // forced_inclusion: Option<BatchParams>,
) -> Result<TransactionRequest, Error> {

    let mut bytes = Bytes::new();

    let mut blocks = Vec::with_capacity(l2_blocks.len());
    for l2_block in l2_blocks {
        let block_manifest = lib_manifest::BlockManifest {
            timestamp: Uint::<48, 1>::from(l2_block.timestamp_sec),
            coinbase: coinbase,
            anchorBlockNumber: Uint::<48, 1>::from(last_anchor_origin_height),
            gasLimit: Uint::<48, 1>::from(0), //TODO l2_block.gas_limit, should block have a gas limit?
            transactions: l2_block
                .prebuilt_tx_list
                .tx_list
                .iter()
                .map(|tx| create_signed_transaction(tx))
                .collect::<Result<Vec<_>, Error>>()?,
        };

        blocks.push(block_manifest);
    }

    let proposal_manifest = lib_manifest::ProposalManifest {
        proverAuthBytes: bytes,
        blocks: vec![],
    };

    let encoded_proposal_manifest = lib_manifest::ProposalManifest::abi_encode(&proposal_manifest);
    let blob_sidecar = common::blob::build_blob_sidecar(&encoded_proposal_manifest)?;

    let tx = TransactionRequest::default()
        // .with_from(from)
        // .with_to(to)
        .with_blob_sidecar(blob_sidecar)
        .with_call(&iinbox::IInbox::proposeCall {
            _lookahead: Bytes::new(),
            _data: Bytes::new(), // ProposeInput
        });

    Ok(tx)
}

fn create_signed_transaction(
    transaction: &alloy::rpc::types::Transaction,
) -> Result<lib_manifest::SignedTransaction, Error> {
    let access_list = if let Some(access_list) = transaction.access_list() {
        let mut buffer = Vec::new();
        access_list.encode(&mut buffer);
        buffer.into()
    } else {
        Bytes::new()
    };

    let signed_transaction = lib_manifest::SignedTransaction {
        txType: transaction.inner.tx_type().ty(),
        chainId: transaction
            .inner
            .inner()
            .chain_id()
            .ok_or(anyhow::anyhow!("Chain ID not found"))?,
        nonce: transaction.inner.nonce(),
        maxPriorityFeePerGas: Uint::<256, 4>::from(
            transaction
                .inner
                .max_priority_fee_per_gas()
                .ok_or(anyhow::anyhow!("Max priority fee per gas not found"))?,
        ),
        maxFeePerGas: Uint::<256, 4>::from(transaction.inner.max_fee_per_gas()),
        gasLimit: transaction.inner.gas_limit(),
        to: transaction
            .inner
            .to()
            .ok_or(anyhow::anyhow!("To not found"))?,
        value: transaction.inner.value(),
        data: transaction.inner.input().clone(),
        accessList: access_list,
        v: transaction.inner.signature().v().into(),
        r: transaction.inner.signature().r().into(),
        s: transaction.inner.signature().s().into(),
    };

    Ok(signed_transaction)
}
