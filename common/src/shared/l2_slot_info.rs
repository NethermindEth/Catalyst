use alloy::primitives::B256;

pub struct L2SlotInfo {
    base_fee: u64,
    slot_timestamp: u64,
    parent_id: u64,
    parent_hash: B256,
    parent_gas_used: u32,
}

impl L2SlotInfo {
    pub fn new(
        base_fee: u64,
        slot_timestamp: u64,
        parent_id: u64,
        parent_hash: B256,
        parent_gas_used: u32,
    ) -> Self {
        Self {
            base_fee,
            slot_timestamp,
            parent_id,
            parent_hash,
            parent_gas_used,
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

    pub fn parent_gas_used(&self) -> u32 {
        self.parent_gas_used
    }
}
