//TODO: allow unused code until ProposalBuilder is used
#![allow(unused)]

use super::{
    bindings::{ICodec, IInbox},
    proposal::Proposal,
};
use alloy::{
    network::{TransactionBuilder, TransactionBuilder4844},
    primitives::{Address, Bytes},
    providers::DynProvider,
    rpc::types::TransactionRequest,
};
use anyhow::Error;
use common::shared::l2_block::L2Block;

struct ProposalBuilder {
    provider: DynProvider,
    codec_address: Address,
}

impl ProposalBuilder {
    pub fn new(provider: DynProvider, codec_address: Address) -> Self {
        Self {
            provider,
            codec_address,
        }
    }

    pub async fn build_proposal_tx(
        &self,
        l2_blocks: Vec<L2Block>,
        last_anchor_origin_height: u64,
        coinbase: Address,
        _current_l1_slot_timestamp: u64,
        from: Address,
        to: Address,
        // forced_inclusion: Option<BatchParams>, // TODO: add forced inclusion
    ) -> Result<TransactionRequest, Error> {
        let proposal = Proposal::build(l2_blocks, last_anchor_origin_height, coinbase)?;
        let blob_sidecar = common::blob::build_blob_sidecar(&proposal.blob_data)?;

        let codec = ICodec::new(self.codec_address, self.provider.clone());
        let encoded_proposal_input = codec
            .encodeProposeInput(proposal.propose_input)
            .call()
            .await?;

        let tx = TransactionRequest::default()
            .with_from(from)
            .with_to(to)
            .with_blob_sidecar(blob_sidecar)
            .with_call(&IInbox::proposeCall {
                _lookahead: Bytes::new(),
                _data: encoded_proposal_input,
            });

        Ok(tx)
    }
}
