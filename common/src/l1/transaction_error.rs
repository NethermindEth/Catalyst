#[derive(Debug, Clone)]
pub enum TransactionError {
    BuildFailed,
    EstimationFailed,
    EstimationTooEarly,
    TransactionReverted,
    NotConfirmed,
    UnsupportedTransactionType,
    GetBlockNumberFailed,
    InsufficientFunds,
    ReanchorRequired,
    OldestForcedInclusionDue,
    NotTheOperatorInCurrentEpoch,
}

impl std::fmt::Display for TransactionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
