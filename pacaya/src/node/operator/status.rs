#[cfg(feature = "get_status_duration")]
#[cfg_attr(test, derive(Debug))]
pub struct StatusCheckDurations {
    pub check_taiko_wrapper: std::time::Duration,
    pub check_handover_window_slots: std::time::Duration,
    pub check_current_operator: std::time::Duration,
    pub check_handover_window: std::time::Duration,
    pub check_driver_status: std::time::Duration,
    pub check_driver_synced: std::time::Duration,
    pub check_preconfer: std::time::Duration,
    pub check_preconfirmation_started: std::time::Duration,
    pub check_submitter: std::time::Duration,
    pub check_end_of_sequencing: std::time::Duration,
}

#[cfg_attr(test, derive(Debug))]
pub struct Status {
    preconfer: bool,
    submitter: bool,
    preconfirmation_started: bool,
    end_of_sequencing: bool,
    is_driver_synced: bool,
    #[cfg(feature = "get_status_duration")]
    durations: Option<StatusCheckDurations>,
}

#[cfg(test)]
impl PartialEq for Status {
    fn eq(&self, other: &Self) -> bool {
        self.preconfer == other.preconfer
            && self.submitter == other.submitter
            && self.preconfirmation_started == other.preconfirmation_started
            && self.end_of_sequencing == other.end_of_sequencing
            && self.is_driver_synced == other.is_driver_synced
    }
}

impl Status {
    pub fn new(
        preconfer: bool,
        submitter: bool,
        preconfirmation_started: bool,
        end_of_sequencing: bool,
        is_driver_synced: bool,
        #[cfg(feature = "get_status_duration")] durations: Option<StatusCheckDurations>,
    ) -> Self {
        Self {
            preconfer,
            submitter,
            preconfirmation_started,
            end_of_sequencing,
            is_driver_synced,
            #[cfg(feature = "get_status_duration")]
            durations,
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

    #[cfg(feature = "get_status_duration")]
    pub fn get_durations(&self) -> Option<&StatusCheckDurations> {
        self.durations.as_ref()
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
