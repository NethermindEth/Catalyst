use taiko_bindings::anchor::LibBonds::BondInstruction;
use alloy::primitives::{ B256};

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