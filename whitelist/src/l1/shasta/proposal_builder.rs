use alloy::{
    network::{TransactionBuilder, TransactionBuilder4844},
    primitives::{Address, Bytes},
    rpc::types::TransactionRequest,
};
use anyhow::Error;
use common::{l1::shasta::proposal::Proposal, shared::l2_block::L2Block};

pub async fn build_proposal_tx(
    // &self,
    l2_blocks: Vec<L2Block>,
    last_anchor_origin_height: u64,
    coinbase: Address,
    current_l1_slot_timestamp: u64,
    // forced_inclusion: Option<BatchParams>,
) -> Result<TransactionRequest, Error> {
    let proposal = Proposal::new();
    let data = proposal.build_blob_data(l2_blocks, last_anchor_origin_height, coinbase)?;
    let blob_sidecar = common::blob::build_blob_sidecar(&data)?;

    let tx = TransactionRequest::default()
        // .with_from(from)
        // .with_to(to)
        .with_blob_sidecar(blob_sidecar)
        // .with_call(&iinbox::IInbox::proposeCall {
        //     _lookahead: Bytes::new(),
        //     _data: Bytes::new(), // ProposeInput
        // })
        ;

    Ok(tx)
}
