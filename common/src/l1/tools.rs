use super::transaction_error::TransactionError;

// shasta
// 0xe92c469f -> InsufficientBond()
pub fn check_for_insufficient_funds(err_str: &str) -> bool {
    err_str.contains("insufficient funds")
        || err_str.contains("insufficient allowance")
        || err_str.contains("0xe92c469f")
}

/// 0x46afbf54 -> AnchorBlockIdTooSmall()
/// 0x1999aed2 -> TimestampTooSmall()
/// 0xfe1698b2 -> AnchorBlockIdSmallerThanParent()
/// 0x21389b84 -> TimestampSmallerThanParent()
pub fn check_for_reanchor_required(err_str: &str) -> bool {
    err_str.contains("0x46afbf54")
        || err_str.contains("0x1999aed2")
        || err_str.contains("0xfe1698b2")
        || err_str.contains("0x21389b84")
}

/// 0x3d32ffdb -> TimestampTooLarge()
/// 0x2b44f010 -> ZeroAnchorBlockHash()
/// shasta
/// 0x92a2f43a -> CannotProposeInCurrentBlock()
pub fn check_for_too_early_estimation(err_str: &str) -> bool {
    err_str.contains("0x3d32ffdb")
        || err_str.contains("0x2b44f010")
        || err_str.contains("0x92a2f43a")
}

// 0x1e66a770 -> OldestForcedInclusionDue()
// shasta
// 0x02967fb6 -> UnprocessedForcedInclusionIsDue()
pub fn check_oldest_forced_inclusion_due(err_str: &str) -> bool {
    err_str.contains("0x1e66a770") || err_str.contains("0x02967fb6")
}

// 0x47fac6c1 -> NotTheOperator()
// 0x795e2f19 -> NotPreconfer()
// 0xc0ec4b50 -> NotPreconferOrFallback()
// shasta
// 0x4100ac03 -> InvalidProposer()
pub fn check_for_not_the_operator_in_current_epoch(err_str: &str) -> bool {
    // TODO: for new contracts version we should remove NotTheOperator
    // as it was renamed to NotPreconfer
    err_str.contains("0x47fac6c1")
        || err_str.contains("0x795e2f19")
        || err_str.contains("0xc0ec4b50")
        || err_str.contains("0x4100ac03")
}

pub fn convert_error_payload(err: &str) -> Option<TransactionError> {
    if check_for_too_early_estimation(err) {
        return Some(TransactionError::EstimationTooEarly);
    }
    if check_for_insufficient_funds(err) {
        return Some(TransactionError::InsufficientFunds);
    }
    if check_for_reanchor_required(err) {
        return Some(TransactionError::ReanchorRequired);
    }
    if check_oldest_forced_inclusion_due(err) {
        return Some(TransactionError::OldestForcedInclusionDue);
    }
    if check_for_not_the_operator_in_current_epoch(err) {
        return Some(TransactionError::NotTheOperatorInCurrentEpoch);
    }
    None
}
