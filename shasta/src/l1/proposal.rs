// TODO remove allow dead_code when the module is used
#![allow(dead_code)]

use super::bindings::iinbox;
use super::bindings::lib_manifest;
use alloy::{
    consensus::Transaction,
    eips::Typed2718,
    primitives::{Address, Bytes, FixedBytes, Uint},
    rlp::Encodable,
};
use anyhow::Error;
use common::{blob::constants::MAX_BLOB_DATA_SIZE, shared::l2_block::L2Block};
use flate2::{Compression, write::ZlibEncoder};
use std::io::Write;

pub struct Proposal {
    pub blob_data: Vec<u8>,
    pub propose_input: iinbox::IInbox::ProposeInput,
}

impl Proposal {
    pub fn build(
        l2_blocks: Vec<L2Block>,
        last_anchor_origin_height: u64,
        coinbase: Address,
    ) -> Result<Self, Error> {
        let blob_data = Self::build_blob_data(l2_blocks, last_anchor_origin_height, coinbase)?;
        let num_blobs = u16::try_from(blob_data.len() / MAX_BLOB_DATA_SIZE)?;
        let propose_input = Self::construct_propose_input(num_blobs)?;
        Ok(Self {
            blob_data,
            propose_input,
        })
    }

    fn construct_propose_input(num_blobs: u16) -> Result<iinbox::IInbox::ProposeInput, Error> {
        let core_state = iinbox::IInbox::CoreState {
            nextProposalId: Uint::<48, 1>::from(0),
            nextProposalBlockId: Uint::<48, 1>::from(0),
            lastFinalizedProposalId: Uint::<48, 1>::from(0),
            lastFinalizedTransitionHash: FixedBytes::from([0u8; 32]),
            bondInstructionsHash: FixedBytes::from([0u8; 32]),
        };

        // starting from first blob, don't need additional offset before actual data
        let blob_reference = iinbox::LibBlobs::BlobReference {
            blobStartIndex: 0u16,
            numBlobs: num_blobs,
            offset: Uint::<24, 1>::from(0),
        };

        let checkpoint = iinbox::ICheckpointStore::Checkpoint {
            blockNumber: Uint::<48, 1>::from(0),
            blockHash: FixedBytes::from([0u8; 32]),
            stateRoot: FixedBytes::from([0u8; 32]),
        };

        let propose_input = iinbox::IInbox::ProposeInput {
            deadline: Uint::<48, 1>::from(0),
            coreState: core_state,
            parentProposals: vec![],
            blobReference: blob_reference,
            transitionRecords: vec![],
            checkpoint,
            numForcedInclusions: 0,
        };

        Ok(propose_input)
    }

    pub fn build_blob_data(
        l2_blocks: Vec<L2Block>,
        last_anchor_origin_height: u64,
        coinbase: Address,
    ) -> Result<Vec<u8>, Error> {
        let mut blocks = Vec::with_capacity(l2_blocks.len());
        for l2_block in l2_blocks {
            let block_manifest = lib_manifest::BlockManifest {
                timestamp: Uint::<48, 1>::from(l2_block.timestamp_sec),
                coinbase,
                anchorBlockNumber: Uint::<48, 1>::from(last_anchor_origin_height),
                gasLimit: Uint::<48, 1>::from(Self::calculate_block_gas_limit()),
                transactions: l2_block
                    .prebuilt_tx_list
                    .tx_list
                    .iter()
                    .map(Self::create_signed_transaction)
                    .collect::<Result<Vec<_>, Error>>()?,
            };

            blocks.push(block_manifest);
        }

        let proposal_manifest = lib_manifest::ProposalManifest {
            proverAuthBytes: Bytes::new(), // Optional, left empty, not choosing specific prover
            blocks,
        };

        let mut encoded_proposal_manifest = Self::encode_and_compress_manifest(&proposal_manifest)?;
        let manifest_len = u32::try_from(encoded_proposal_manifest.len())?;
        const DEFAULT_SHASTA_VERSION: u32 = 0x01;
        let mut data = Vec::with_capacity(2 * 4 + encoded_proposal_manifest.len());
        data.extend_from_slice(&DEFAULT_SHASTA_VERSION.to_be_bytes());
        data.extend_from_slice(&manifest_len.to_be_bytes());
        data.append(&mut encoded_proposal_manifest);
        Ok(data)
    }

    // RLP encode and zlib compress
    pub fn encode_and_compress_manifest(
        proposal_manifest: &lib_manifest::ProposalManifest,
    ) -> Result<Vec<u8>, Error> {
        // First RLP encode the proposal manifest
        let mut buffer = Vec::<u8>::new();
        proposal_manifest.encode(&mut buffer);

        // Then compress using zlib
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&buffer).map_err(|e| {
            anyhow::anyhow!(
                "proposal_manifest encode_and_compress: Failed to compress: {}",
                e
            )
        })?;
        encoder.finish().map_err(|e| {
            anyhow::anyhow!(
                "proposal_manifest encode_and_compress: Failed to finish: {}",
                e
            )
        })
    }

    fn calculate_block_gas_limit() -> u64 {
        // const BLOCK_GAS_LIMIT_MAX_CHANGE : u64 = 10;
        // const MIN_BLOCK_GAS_LIMIT:u64 = 15_000_000;
        // let parent_gas_limit = 0; // TODO take it from parent.metadata.gas_limit
        // let lower_bound = std::cmp::max(
        //     parent_gas_limit * (10000 - BLOCK_GAS_LIMIT_MAX_CHANGE) / 10000,
        //     MIN_BLOCK_GAS_LIMIT,
        // );
        // let  upperBound = parent_gas_limit * (10000 + BLOCK_GAS_LIMIT_MAX_CHANGE) / 10000;

        // TODO returning 0 until we have parent gas limit to enable above calculation
        0
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

    pub fn send() {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{Address, Uint};
    use common::shared::l2_block::L2Block;
    use common::shared::l2_tx_lists::PreBuiltTxList;

    // Helper function to create a test L2 block with empty transactions
    fn create_test_l2_block(timestamp_sec: u64, num_transactions: usize) -> L2Block {
        let prebuilt_tx_list = PreBuiltTxList {
            tx_list: vec![], // Empty transaction list for simplicity
            estimated_gas_used: 21000 * num_transactions as u64,
            bytes_length: 100 * num_transactions as u64, // Approximate
        };

        L2Block::new_from(prebuilt_tx_list, timestamp_sec)
    }

    #[test]
    fn test_proposal_build_happy_path() {
        // Test the main happy path: building a proposal with multiple L2 blocks
        let l2_blocks = vec![
            create_test_l2_block(1234567890, 1),
            create_test_l2_block(1234567891, 2),
            create_test_l2_block(1234567892, 0),
        ];
        let last_anchor_origin_height = 1000;
        let coinbase = Address::from([1u8; 20]);

        let result = Proposal::build(l2_blocks, last_anchor_origin_height, coinbase);
        assert!(result.is_ok(), "Proposal build should succeed");

        let proposal = result.unwrap();

        // Verify blob data structure
        assert!(
            !proposal.blob_data.is_empty(),
            "Blob data should not be empty"
        );
        assert!(
            proposal.blob_data.len() >= 8,
            "Blob data should have at least version + length header"
        );

        // Check version and length header
        let version = u32::from_be_bytes([
            proposal.blob_data[0],
            proposal.blob_data[1],
            proposal.blob_data[2],
            proposal.blob_data[3],
        ]);
        assert_eq!(
            version, 0x01,
            "Version should be DEFAULT_SHASTA_VERSION (0x01)"
        );

        let manifest_len = u32::from_be_bytes([
            proposal.blob_data[4],
            proposal.blob_data[5],
            proposal.blob_data[6],
            proposal.blob_data[7],
        ]);
        assert!(manifest_len > 0, "Manifest length should be greater than 0");

        // Verify propose input structure
        assert_eq!(
            proposal.propose_input.blobReference.blobStartIndex, 0,
            "Blob start index should be 0"
        );
        assert_eq!(
            proposal.propose_input.blobReference.offset,
            Uint::<24, 1>::from(0),
            "Blob offset should be 0"
        );
    }
}
