#[derive(Debug, PartialEq, Eq)]
pub struct Status {
    preconfer: bool,
    submitter: bool,
    preconfirmation_started: bool,
    end_of_sequencing: bool,
    is_driver_synced: bool,
}

impl Status {
    pub fn new(
        preconfer: bool,
        submitter: bool,
        preconfirmation_started: bool,
        end_of_sequencing: bool,
        is_driver_synced: bool,
    ) -> Self {
        Self {
            preconfer,
            submitter,
            preconfirmation_started,
            end_of_sequencing,
            is_driver_synced,
        }
    }

    pub fn is_preconfer(&self) -> bool {
        self.preconfer
    }

    pub fn is_submitter(&self) -> bool {
        self.submitter
    }

    pub fn is_driver_synced(&self) -> bool {
        self.is_driver_synced
    }

    pub fn is_preconfirmation_start_slot(&self) -> bool {
        self.preconfirmation_started
    }

    pub fn is_end_of_sequencing(&self) -> bool {
        self.end_of_sequencing
    }
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut roles = Vec::new();

        if self.preconfer {
            roles.push("Preconf");
        }

        if self.submitter {
            roles.push("Submit");
        }

        if self.preconfer && self.is_driver_synced {
            roles.push("Synced");
        }

        if self.end_of_sequencing {
            roles.push("EndOfSequencing");
        }

        if roles.is_empty() {
            write!(f, "No active roles")
        } else {
            write!(f, "{}", roles.join(", "))
        }
    }
}
