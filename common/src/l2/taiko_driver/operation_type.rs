use std::fmt;

#[derive(Copy, Clone)]
pub enum OperationType {
    Preconfirm,
    Reanchor,
    ReorgStaleBlock,
    Status,
}

impl fmt::Display for OperationType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            OperationType::Preconfirm => "Preconfirm",
            OperationType::Reanchor => "Reanchor",
            OperationType::ReorgStaleBlock => "ReorgStaleBlock",
            OperationType::Status => "Status",
        };
        write!(f, "{s}")
    }
}
