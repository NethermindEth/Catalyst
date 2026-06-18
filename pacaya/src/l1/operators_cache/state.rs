use alloy::primitives::Address;

#[derive(Clone, Debug)]
pub struct Operators {
    pub current: Address,
    pub next: Address,
}

#[derive(Clone, Debug)]
pub struct OperatorsCacheState {
    timestamp: u64,
    operators: Operators,
}

impl OperatorsCacheState {
    pub fn new(timestamp: u64, current: Address, next: Address) -> Self {
        Self {
            timestamp,
            operators: Operators { current, next },
        }
    }

    pub fn timestamp(&self) -> u64 {
        self.timestamp
    }

    pub fn current_operator(&self) -> Address {
        self.operators.current
    }

    pub fn next_operator(&self) -> Address {
        self.operators.next
    }
}
