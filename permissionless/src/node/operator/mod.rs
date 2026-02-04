mod status;

use anyhow::Error;
use common::shared::l2_slot_info::SlotData;
use status::Status;

use taiko_protocol::preconfirmation::lookahead::LookaheadResolver;

struct Operator {
    lookahead_resolver: LookaheadResolver,
}

impl Operator {
    pub fn new(lookahead_resolver: LookaheadResolver) -> Self {
        Self { lookahead_resolver }
    }

    pub fn get_status<S: SlotData>(&self, l2_slot_info: S) -> Result<Status, Error> {
        // pobrać adress obecnego oparatora
        // pobrać addreses następnego

        Ok(Status::new(false, false))
    }
}
