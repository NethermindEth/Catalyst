mod status;

use anyhow::Error;
use common::shared::l2_slot_info::SlotData;
use status::Status;
struct Operator {}

impl Operator {
    pub fn new() {}

    pub fn get_status<S: SlotData>(&self,s l2_slot_info: S) -> Result<Status, Error> {
        // pobrać adress obecnego oparatora
        // pobrać addreses następnego
    }
}
