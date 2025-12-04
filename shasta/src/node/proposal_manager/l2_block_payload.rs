use crate::shared::l2_tx_lists::PreBuiltTxList;
use alloy::primitives::B256;
use crate::node::proposal_manager::bond_instruction_data::BondInstructionData;

pub struct L2BlockV2Payload {
    pub proposal_id: u64,
    pub block_id: u64,
    pub coinbase: alloy::primitives::Address,
    pub prebuilt_tx_list: PreBuiltTxList,
    pub timestamp_sec: u64,
    pub gas_limit_without_anchor: u64,
    pub anchor_block_id: u64,
    pub anchor_block_hash: B256,
    pub anchor_state_root: B256,
    pub bond_instructions: BondInstructionData,
    pub base_fee_per_gas: u64,
}