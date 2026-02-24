pub mod status;

use crate::l2::preconfirmation_driver::PreconfirmationDriver;
use alloy::primitives::{Address, U256};
use anyhow::Error;
use common::shared::l2_slot_info_v2::L2SlotInfoV2;
use status::Status;
use std::sync::Arc;

pub struct Operator {
    driver: Arc<PreconfirmationDriver>,
    preconfer_address: Address,
}

impl Operator {
    pub fn new(driver: Arc<PreconfirmationDriver>, preconfer_address: Address) -> Self {
        Self {
            driver,
            preconfer_address,
        }
    }

    pub async fn get_status(&self, l2_slot_info: L2SlotInfoV2) -> Result<Status, Error> {
        let preconf_slot_info = self
            .driver
            .get_preconf_slot_info(U256::from(l2_slot_info.slot_timestamp()))
            .await?;

        let preconfer = preconf_slot_info.signer == self.preconfer_address;

        Ok(Status::new(preconfer, false))
    }

    pub fn preconfirmation_driver(&self) -> &PreconfirmationDriver {
        &self.driver
    }
}
