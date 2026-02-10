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

    #[allow(unused)]
    pub fn is_preconfer(&self) -> bool {
        self.preconfer
    }

    #[allow(unused)]
    pub fn is_proposer(&self) -> bool {
        self.proposer
    }
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut roles = Vec::new();

        if self.preconfer {
            roles.push("Preconf");
        }

        if self.proposer {
            roles.push("Proposer");
        }

        if roles.is_empty() {
            write!(f, "No active roles")
        } else {
            write!(f, "{}", roles.join(", "))
        }
    }
}
