use crate::shared::l2_block_v2::L2BlockV2;
use alloy::primitives::{Address, B256, Bytes};
use std::collections::VecDeque;
use std::time::Instant;
use taiko_bindings::anchor::LibBonds::BondInstruction;
use taiko_protocol::shasta::manifest::{BlockManifest, DerivationSourceManifest};
use tracing::{debug, warn};
use crate::shared::l2_tx_lists::PreBuiltTxList;

pub type Proposals = VecDeque<Proposal>;

#[derive(Default, Clone)]
pub struct BondInstructionData {
    instructions: Vec<BondInstruction>,
    hash: B256,
}

impl BondInstructionData {
    pub fn new(instructions: Vec<BondInstruction>, hash: B256) -> Self {
        Self { instructions, hash }
    }

    pub fn instructions(&self) -> &Vec<BondInstruction> {
        &self.instructions
    }

    pub fn instructions_mut(self) -> Vec<BondInstruction> {
        self.instructions
    }

    pub fn hash(&self) -> B256 {
        self.hash
    }
}

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
    pub bond_instructions: BondInstructionData,
    pub num_forced_inclusion: u8,
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
                gas_limit: l2_block.gas_limit,
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
            prover_auth_bytes: Bytes::new(),
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

    pub fn get_last_block_timestamp(&self) -> Result<u64, anyhow::Error> {
        self.l2_blocks
            .last()
            .map(|block| block.timestamp_sec)
            .ok_or_else(|| anyhow::anyhow!("No L2 blocks in proposal"))
    }

    pub fn has_only_one_common_block(&self) -> bool {
        self.num_forced_inclusion == 0 && self.l2_blocks.len() == 1
    }

    pub fn is_empty(&self) -> bool {
        self.num_forced_inclusion == 0 && self.l2_blocks.is_empty()
    }

    pub fn get_last_block_tx_list_copy(
        &self,
    ) -> Result<Vec<alloy::rpc::types::Transaction>, anyhow::Error> {
        self.l2_blocks
            .last()
            .map(|block| block.prebuilt_tx_list.tx_list.clone())
            .ok_or_else(|| anyhow::anyhow!("No L2 blocks in proposal"))
    }

    pub fn get_last_block_tx_len(&self) -> Result<usize, anyhow::Error> {
        self.l2_blocks
            .last()
            .map(|block| block.prebuilt_tx_list.tx_list.len())
            .ok_or_else(|| anyhow::anyhow!("No L2 blocks in proposal"))
    }

    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    pub fn add_l2_block(&mut self, tx_list: PreBuiltTxList, timestamp_sec: u64, gas_limit: u64) {
        self.total_bytes += tx_list.bytes_length;
        let l2_block = L2BlockV2::new_from(
            tx_list,
            timestamp_sec,
            self.coinbase,
            self.anchor_block_id,
            gas_limit,
        );
        self.l2_blocks.push(l2_block);
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

        let l2_block = L2Block {
            prebuilt_tx_list: PreBuiltTxList {
                tx_list: vec![tx],
                estimated_gas_used: 0,
                bytes_length: 0,
            },
            timestamp_sec: 0,
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
            bond_instructions: BondInstructionData::default(),
            num_forced_inclusion: 0,
        };

        proposal.compress();

        assert!(proposal.total_bytes == 316);
        assert!(buffer.len() > proposal.total_bytes as usize);
    }
}
