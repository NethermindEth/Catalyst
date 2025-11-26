//use pacaya::node::operator::Status as OperatorStatus;
use common::shared::l2_tx_lists::PreBuiltTxList;
use common::shared::l2_slot_info::L2SlotInfo;

pub struct L2SlotContext {
    pub pending_tx_list:  Option<PreBuiltTxList>,
    pub info: L2SlotInfo,
    pub is_end_of_sequencing: bool,
    pub allow_forced_inclusion: bool,
}

impl L2SlotContext {
    pub fn new(
        pending_tx_list: Option<PreBuiltTxList>,
        info: L2SlotInfo,
        is_end_of_sequencing: bool,
        allow_forced_inclusion: bool,
    ) -> Self {
        Self {
            pending_tx_list,
            info,
            is_end_of_sequencing,
            allow_forced_inclusion,
        }
    }
}
