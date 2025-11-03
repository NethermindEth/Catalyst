use crate::shared::l2_block::L2Block;
use alloy::primitives::{Address, B256};
use taiko_bindings::anchor::LibBonds::BondInstruction;

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

    pub fn hash(&self) -> B256 {
        self.hash
    }
}

#[derive(Default, Clone)]
pub struct Proposal {
    pub id: u64,
    pub l2_blocks: Vec<L2Block>,
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
        // TODO implement proper compression
    }

    pub fn get_last_block_timestamp(&self) -> Result<u64, anyhow::Error> {
        self.l2_blocks
            .last()
            .map(|block| block.timestamp_sec)
            .ok_or_else(|| anyhow::anyhow!("No L2 blocks in proposal"))
    }

    pub fn has_only_one_block(&self) -> bool {
        self.l2_blocks.len() == 1
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
}
