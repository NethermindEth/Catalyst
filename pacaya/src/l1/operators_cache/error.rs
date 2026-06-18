#[derive(Debug)]
pub enum OperatorsCacheError {
    LatestBlockFetchFailed { source: String },
    LatestBlockNotFound,
    RpcBehindCurrentSlot { block_timestamp: u64 },
    CurrentOperatorFetchFailed { source: String },
    NextOperatorFetchFailed { source: String },
}

impl std::fmt::Display for OperatorsCacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LatestBlockFetchFailed { source } => {
                write!(f, "Failed to get latest block: {}", source)
            }
            Self::LatestBlockNotFound => write!(f, "No latest block found"),
            Self::RpcBehindCurrentSlot { block_timestamp } => {
                write!(f, "RPC behind current slot (latest: {})", block_timestamp)
            }
            Self::CurrentOperatorFetchFailed { source } => {
                write!(f, "Failed to get current operator: {}", source)
            }
            Self::NextOperatorFetchFailed { source } => {
                write!(f, "Failed to get next operator: {}", source)
            }
        }
    }
}

impl std::error::Error for OperatorsCacheError {}
