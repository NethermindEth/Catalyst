use crate::shared::l2_tx_lists::PreBuiltTxList;

#[derive(Debug, Clone)]
pub struct L2Block {
    pub prebuilt_tx_list: PreBuiltTxList,
    pub timestamp_sec: u64,
}

impl L2Block {
    pub fn new_from(tx_list: PreBuiltTxList, timestamp_sec: u64) -> Self {
        L2Block {
            prebuilt_tx_list: tx_list,
            timestamp_sec,
        }
    }

    pub fn new_empty(timestamp_sec: u64) -> Self {
        L2Block {
            prebuilt_tx_list: PreBuiltTxList::empty(),
            timestamp_sec,
        }
    }
}
