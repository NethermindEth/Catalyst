pub struct Status {
    preconfer: bool,
    proposer: bool,
}

impl Status {
    pub fn new(preconfer: bool, proposer: bool) -> Self {
        Self {
            preconfer,
            proposer,
        }
    }

    pub fn is_preconfer(&self) -> bool {
        self.preconfer
    }

    pub fn is_proposer(&self) -> bool {
        self.proposer
    }
}
