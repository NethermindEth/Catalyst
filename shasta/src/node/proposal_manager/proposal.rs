use crate::node::proposal_manager::l2_block_payload::L2BlockV2Payload;
use alloy::primitives::{Address, B256};
use common::shared::l2_block_v2::{L2BlockV2, L2BlockV2Draft};
use std::collections::VecDeque;
use std::time::Instant;
use taiko_bindings::anchor::ICheckpointStore::Checkpoint;
use taiko_protocol::shasta::manifest::{BlockManifest, DerivationSourceManifest};
use tracing::{debug, warn};

pub type Proposals = VecDeque<Proposal>;

#[derive(Default, Clone)]
pub struct Proposal {
    pub id: u64,
    pub l2_blocks: Vec<L2BlockV2>,
    pub total_bytes: u64,
    pub coinbase: Address,
    pub anchor_block_id: u64,
    pub anchor_block_timestamp_sec: u64,
    pub anchor_block_hash: B256,
    pub anchor_state_root: B256,
    pub num_forced_inclusion: u16,
}

impl Proposal {
    pub fn compress(&mut self) {
        let start = Instant::now();

        let mut block_manifests = <Vec<BlockManifest>>::with_capacity(self.l2_blocks.len());
        for l2_block in &self.l2_blocks {
            // Build the block manifests.
            block_manifests.push(BlockManifest {
                timestamp: l2_block.timestamp_sec,
                coinbase: l2_block.coinbase,
                anchor_block_number: l2_block.anchor_block_number,
                gas_limit: l2_block.gas_limit_without_anchor,
                transactions: l2_block
                    .prebuilt_tx_list
                    .tx_list
                    .iter()
                    .map(|tx| tx.clone().into())
                    .collect(),
            });
        }

        // Build the proposal manifest.
        let manifest = DerivationSourceManifest {
            blocks: block_manifests,
        };

        let manifest_data = match manifest.encode_and_compress() {
            Ok(data) => data,
            Err(err) => {
                warn!("Failed to compress proposal manifest: {err}");
                return;
            }
        };

        debug!(
            "Proposal compression completed in {} ms. Total bytes before: {}. Total bytes after: {}.",
            start.elapsed().as_millis(),
            self.total_bytes,
            manifest_data.len()
        );

        self.total_bytes = manifest_data.len() as u64;
    }

    fn create_block_from_draft(&mut self, l2_draft_block: L2BlockV2Draft) -> L2BlockV2 {
        L2BlockV2::new_from(
            l2_draft_block.prebuilt_tx_list,
            l2_draft_block.timestamp_sec,
            self.coinbase,
            self.anchor_block_id,
            l2_draft_block.gas_limit_without_anchor,
        )
    }

    pub fn add_forced_inclusion(
        &mut self,
        fi_block: L2BlockV2Draft,
        anchor_params: Checkpoint,
    ) -> L2BlockV2Payload {
        let l2_payload = L2BlockV2Payload {
            proposal_id: self.id,
            coinbase: self.coinbase,
            tx_list: fi_block.prebuilt_tx_list.tx_list,
            timestamp_sec: fi_block.timestamp_sec,
            gas_limit_without_anchor: fi_block.gas_limit_without_anchor,
            anchor_block_id: anchor_params.blockNumber.to::<u64>(),
            anchor_block_hash: anchor_params.blockHash,
            anchor_state_root: anchor_params.stateRoot,
            is_forced_inclusion: true,
        };
        self.num_forced_inclusion += 1;
        l2_payload
    }

    pub fn add_l2_block(&mut self, l2_block: L2BlockV2) -> L2BlockV2Payload {
        let l2_payload = L2BlockV2Payload {
            proposal_id: self.id,
            coinbase: self.coinbase,
            tx_list: l2_block.prebuilt_tx_list.tx_list.clone(),
            timestamp_sec: l2_block.timestamp_sec,
            gas_limit_without_anchor: l2_block.gas_limit_without_anchor,
            anchor_block_id: self.anchor_block_id,
            anchor_block_hash: self.anchor_block_hash,
            anchor_state_root: self.anchor_state_root,
            is_forced_inclusion: false,
        };
        self.total_bytes += l2_block.prebuilt_tx_list.bytes_length;
        self.l2_blocks.push(l2_block);
        l2_payload
    }

    pub fn add_l2_draft_block(&mut self, l2_draft_block: L2BlockV2Draft) -> L2BlockV2Payload {
        let l2_block = self.create_block_from_draft(l2_draft_block);
        self.add_l2_block(l2_block)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use common::shared::l2_tx_lists::{PreBuiltTxList, rlp_encode};

    #[test]
    fn test_proposal_compression() {
        let json_data = r#"
        {
            "blockHash":"0x845049a264a004a223db6a4b87434cc9b6410f12ff5a15d18fea0d2d04ebb6f2",
            "blockNumber":"0x2",
            "from":"0x0000777735367b36bc9b61c50022d9d0700db4ec",
            "gas":"0xf4240",
            "gasPrice":"0x1243554",
            "maxFeePerGas":"0x1243554",
            "maxPriorityFeePerGas":"0x0",
            "hash":"0x0665b09b818404dec58b96a7a97b44ce4546985e05aacbcdada94ebcab293455",
            "input":"0x100f75880000000000000000000000000000000000000000000000000000000000000080000000000000000000000000000000000000000000000000000000000000001dc836dffc57b4cd0d44c57ccd909e8d03bf21aa153412eab9819b1bb0590cd5606b2ac17d285f8694d0cf3488aaf5e1216315351589fd437899ec83b6091bdc350000000000000000000000000000000000000000000000000000000000000001000000000000000000000000f39fd6e51aad88f6f4ce6ab8827279cfffb9226600000000000000000000000000000000000000000000000000000000000000a0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000c000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
            "nonce":"0x1",
            "to":"0x1670010000000000000000000000000000010001",
            "transactionIndex":"0x0",
            "value":"0x0",
            "type":"0x2",
            "accessList":[],
            "chainId":"0x28c59",
            "v":"0x1",
            "r":"0x79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
            "s":"0xa93618a76a3553d8fd6fa9aa428ff10dc2556107180f37d51754a504c7754d6",
            "yParity":"0x1"
        }"#;

        let tx: alloy::rpc::types::Transaction = serde_json::from_str(json_data).unwrap();

        let l2_block = L2BlockV2 {
            prebuilt_tx_list: PreBuiltTxList {
                tx_list: vec![tx],
                estimated_gas_used: 0,
                bytes_length: 0,
            },
            timestamp_sec: 0,
            coinbase: Address::ZERO,
            anchor_block_number: 0,
            gas_limit_without_anchor: 0,
        };

        // RLP encode the transactions
        let buffer = rlp_encode(&l2_block.prebuilt_tx_list.tx_list);

        let mut proposal = Proposal {
            id: 0,
            l2_blocks: vec![l2_block],
            total_bytes: 0,
            coinbase: Address::ZERO,
            anchor_block_id: 0,
            anchor_block_timestamp_sec: 0,
            anchor_block_hash: B256::ZERO,
            anchor_state_root: B256::ZERO,
            num_forced_inclusion: 0,
        };

        proposal.compress();

        assert!(proposal.total_bytes == 316);
        assert!(buffer.len() > proposal.total_bytes as usize);
    }
}
