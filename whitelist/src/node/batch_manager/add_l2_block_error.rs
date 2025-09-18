use std::fmt;

#[derive(Debug)]
pub enum AddL2BlockError {
    AdvanceHeadError(String),
    BatchError(String),
}

impl fmt::Display for AddL2BlockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AddL2BlockError::AdvanceHeadError(msg) => {
                write!(f, "Failed to advance head to new L2 block: {msg}")
            }
            AddL2BlockError::BatchError(msg) => {
                write!(f, "Failed to add L2 block to batch: {msg}")
            }
        }
    }
}

impl std::error::Error for AddL2BlockError {}
