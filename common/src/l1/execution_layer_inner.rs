use alloy::primitives::Address;

pub struct ExecutionLayerInner {
    chain_id: u64,
    preconfer_address: Address,
}

impl ExecutionLayerInner {
    pub fn new(chain_id: u64, preconfer_address: Address) -> Self {
        Self {
            chain_id,
            preconfer_address,
        }
    }

    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    pub fn preconfer_address(&self) -> Address {
        self.preconfer_address
    }
}
