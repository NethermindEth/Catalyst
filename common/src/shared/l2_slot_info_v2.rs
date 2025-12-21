use super::l2_slot_info::SlotData;
use alloy::primitives::B256;

pub struct L2SlotContext {
    pub info: L2SlotInfoV2,
    pub end_of_sequencing: bool,
    pub allow_forced_inclusion: bool,
}

#[derive(Debug, Clone)]
pub struct L2SlotInfoV2 {
    base_fee: u64,
    slot_timestamp: u64,
    parent_id: u64,
    parent_hash: B256,
    parent_gas_limit_without_anchor: u64,
    parent_timestamp: u64,
}

impl L2SlotInfoV2 {
    pub fn new(
        base_fee: u64,
        slot_timestamp: u64,
        parent_id: u64,
        parent_hash: B256,
        parent_gas_limit_without_anchor: u64,
        parent_timestamp: u64,
    ) -> Self {
        Self {
            base_fee,
            slot_timestamp,
            parent_id,
            parent_hash,
            parent_gas_limit_without_anchor,
            parent_timestamp,
        }
    }

    pub fn new_from_other(other: L2SlotInfoV2, slot_timestamp: u64) -> Self {
        Self {
            base_fee: other.base_fee,
            slot_timestamp,
            parent_id: other.parent_id,
            parent_hash: other.parent_hash,
            parent_gas_limit_without_anchor: other.parent_gas_limit_without_anchor,
            parent_timestamp: other.parent_timestamp,
        }
    }

    pub fn base_fee(&self) -> u64 {
        self.base_fee
    }

    pub fn slot_timestamp(&self) -> u64 {
        self.slot_timestamp
    }

    pub fn parent_id(&self) -> u64 {
        self.parent_id
    }

    pub fn parent_hash(&self) -> &B256 {
        &self.parent_hash
    }

    pub fn parent_gas_limit_without_anchor(&self) -> u64 {
        self.parent_gas_limit_without_anchor
    }

    pub fn parent_timestamp(&self) -> u64 {
        self.parent_timestamp
    }
}

impl SlotData for L2SlotInfoV2 {
    fn slot_timestamp(&self) -> u64 {
        self.slot_timestamp
    }

    fn parent_id(&self) -> u64 {
        self.parent_id
    }

    fn parent_hash(&self) -> &B256 {
        &self.parent_hash
    }
}
