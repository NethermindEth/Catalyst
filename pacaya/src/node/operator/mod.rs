mod status;
mod tests;

use crate::l1::PreconfOperator;
use anyhow::Error;
use common::{
    fork_info::ForkInfo,
    l1::slot_clock::{Clock, SlotClock},
    l2::taiko_driver::{StatusProvider, models::TaikoStatus},
    shared::l2_slot_info::L2SlotInfo,
    utils::{cancellation_token::CancellationToken, types::*},
};
pub use status::Status;
use std::sync::Arc;
use tracing::{debug, warn};

pub struct Operator<T: PreconfOperator, U: Clock, V: StatusProvider> {
    execution_layer: Arc<T>,
    slot_clock: Arc<SlotClock<U>>,
    taiko: Arc<V>,
    handover_window_slots_default: u64,
    handover_window_slots: u64,
    handover_start_buffer_ms: u64,
    next_operator: bool,
    continuing_role: bool,
    simulate_not_submitting_at_the_end_of_epoch: bool,
    was_synced_preconfer: bool,
    cancel_token: CancellationToken,
    cancel_counter: u64,
    operator_transition_slots: u64,
    last_config_reload_epoch: u64,
    fork_info: ForkInfo,
}

const OPERATOR_TRANSITION_SLOTS: u64 = 2;

impl<T: PreconfOperator, U: Clock, V: StatusProvider> Operator<T, U, V> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        execution_layer: Arc<T>,
        slot_clock: Arc<SlotClock<U>>,
        taiko: Arc<V>,
        handover_window_slots: u64,
        handover_start_buffer_ms: u64,
        simulate_not_submitting_at_the_end_of_epoch: bool,
        cancel_token: CancellationToken,
        fork_info: ForkInfo,
    ) -> Result<Self, Error> {
        Ok(Self {
            execution_layer,
            slot_clock,
            taiko,
            handover_window_slots_default: handover_window_slots,
            handover_window_slots,
            handover_start_buffer_ms,
            next_operator: false,
            continuing_role: false,
            simulate_not_submitting_at_the_end_of_epoch,
            was_synced_preconfer: false,
            cancel_token,
            cancel_counter: 0,
            operator_transition_slots: OPERATOR_TRANSITION_SLOTS,
            last_config_reload_epoch: 0,
            fork_info,
        })
    }

    /// Get the current status of the operator based on the current L1 and L2 slots
    pub async fn get_status(&mut self, l2_slot_info: &L2SlotInfo) -> Result<Status, Error> {
        if !self
            .execution_layer
            .is_preconf_router_specified_in_taiko_wrapper()
            .await?
        {
            warn!("PreconfRouter is not specified in TaikoWrapper");
            self.reset();
            return Ok(Status::new(false, false, false, false, false));
        }

        let l1_slot = self.slot_clock.get_current_slot_of_epoch()?;

        let epoch = self.slot_clock.get_current_epoch()?;
        if epoch > self.last_config_reload_epoch {
            self.handover_window_slots = self.get_handover_window_slots().await;
            debug!(
                "Reloaded router config. Handover window slots: {}",
                self.handover_window_slots
            );
            self.last_config_reload_epoch = epoch;
        }

        // For the first N slots of the new epoch, use the next operator from the previous epoch
        // it's because of the delay that L1 updates the current operator after the epoch has changed.
        let current_operator = if l1_slot < self.operator_transition_slots {
            let curr = match self.execution_layer.is_operator_for_current_epoch().await {
                Ok(val) => format!("{val}"),
                Err(e) => {
                    format!("Failed to check current epoch operator: {e}")
                }
            };
            let next = match self.execution_layer.is_operator_for_next_epoch().await {
                Ok(val) => format!("{val}"),
                Err(e) => {
                    format!("Failed to check next epoch operator: {e}")
                }
            };
            tracing::debug!(
                "Status in transition: l1_slot: {} current_operator: {} next_operator: {}",
                l1_slot,
                curr,
                next
            );
            self.next_operator
        } else {
            self.next_operator = match self.execution_layer.is_operator_for_next_epoch().await {
                Ok(val) => val,
                Err(e) => {
                    warn!("Failed to check next epoch operator: {:?}", e);
                    false
                }
            };
            let current_operator = self.execution_layer.is_operator_for_current_epoch().await?;
            self.continuing_role = current_operator && self.next_operator;
            current_operator
        };

        let handover_window = self.is_handover_window(l1_slot);
        let driver_status = self.taiko.get_status().await?;
        let is_driver_synced = self.is_driver_synced(l2_slot_info, &driver_status).await?;
        let preconfer = self
            .is_preconfer(
                current_operator,
                handover_window,
                l1_slot,
                l2_slot_info,
                &driver_status,
            )
            .await?;
        let preconfirmation_started =
            self.is_preconfirmation_start_l2_slot(preconfer, is_driver_synced);
        if preconfirmation_started {
            self.was_synced_preconfer = true;
        }
        if !preconfer {
            self.was_synced_preconfer = false;
        }

        let submitter = self.is_submitter(current_operator, handover_window);
        let end_of_sequencing = self.is_end_of_sequencing(preconfer, submitter, l1_slot)?;

        Ok(Status::new(
            preconfer,
            submitter,
            preconfirmation_started,
            end_of_sequencing,
            is_driver_synced,
        ))
    }

    pub fn reset(&mut self) {
        self.next_operator = false;
        self.continuing_role = false;
        self.was_synced_preconfer = false;
        self.cancel_counter = 0;
    }

    fn is_end_of_sequencing(
        &self,
        preconfer: bool,
        submitter: bool,
        l1_slot: Slot,
    ) -> Result<bool, Error> {
        let slot_before_handover_window = self.is_l2_slot_before_handover_window(l1_slot)?;
        Ok(!self.continuing_role && preconfer && submitter && slot_before_handover_window)
    }

    fn is_l2_slot_before_handover_window(&self, l1_slot: Slot) -> Result<bool, Error> {
        let end_l1_slot = self.slot_clock.get_slots_per_epoch() - self.handover_window_slots - 1;
        if l1_slot == end_l1_slot {
            let l2_slot = self.slot_clock.get_current_l2_slot_within_l1_slot()?;
            Ok(l2_slot + 1 == self.slot_clock.get_number_of_l2_slots_per_l1())
        } else {
            Ok(false)
        }
    }

    async fn is_driver_synced(
        &mut self,
        l2_slot_info: &L2SlotInfo,
        driver_status: &TaikoStatus,
    ) -> Result<bool, Error> {
        let taiko_geth_synced_with_l1 = self.is_taiko_geth_synced_with_l1(l2_slot_info).await?;
        let geth_and_driver_synced = self
            .is_block_height_synced_between_taiko_geth_and_the_driver(driver_status, l2_slot_info)
            .await?;
        if taiko_geth_synced_with_l1 && geth_and_driver_synced {
            self.cancel_counter = 0;
            return Ok(true);
        }

        if !taiko_geth_synced_with_l1 {
            warn!("Taiko Geth is not synced with Taiko inbox height");
        }
        if !geth_and_driver_synced {
            warn!("Geth and driver are not synced");
        }

        self.cancel_counter += 1;
        self.cancel_if_not_synced_for_sufficient_long_time();
        Ok(false)
    }

    async fn is_preconfer(
        &mut self,
        current_operator: bool,
        handover_window: bool,
        l1_slot: Slot,
        l2_slot_info: &L2SlotInfo,
        driver_status: &TaikoStatus,
    ) -> Result<bool, Error> {
        if self
            .fork_info
            .is_fork_switch_transition_period(std::time::Duration::from_secs(
                l2_slot_info.slot_timestamp(),
            ))
        {
            return Ok(false);
        }

        if handover_window {
            return Ok(self.next_operator
                && (self.was_synced_preconfer // If we were the operator for the previous slot, the handover buffer doesn't matter.
                    || !self.is_handover_buffer(l1_slot, l2_slot_info, driver_status).await?));
        }

        Ok(current_operator)
    }

    fn cancel_if_not_synced_for_sufficient_long_time(&mut self) {
        if self.cancel_counter > self.slot_clock.get_l2_slots_per_epoch() / 2 {
            warn!(
                "Not synchronized Geth driver count: {}, exiting...",
                self.cancel_counter
            );
            self.cancel_token.cancel_on_critical_error();
        }
    }

    async fn is_handover_buffer(
        &self,
        l1_slot: Slot,
        l2_slot_info: &L2SlotInfo,
        driver_status: &TaikoStatus,
    ) -> Result<bool, Error> {
        if self.get_ms_from_handover_window_start(l1_slot)? <= self.handover_start_buffer_ms {
            tracing::debug!(
                "Is handover buffer, end_of_sequencing_block_hash: {}",
                driver_status.end_of_sequencing_block_hash
            );
            return Ok(!self.end_of_sequencing_marker_received(driver_status, l2_slot_info));
        }

        Ok(false)
    }

    fn end_of_sequencing_marker_received(
        &self,
        driver_status: &TaikoStatus,
        l2_slot_info: &L2SlotInfo,
    ) -> bool {
        *l2_slot_info.parent_hash() == driver_status.end_of_sequencing_block_hash
    }

    fn is_submitter(&self, current_operator: bool, handover_window: bool) -> bool {
        if handover_window && self.simulate_not_submitting_at_the_end_of_epoch {
            return false;
        }

        current_operator
    }

    fn is_preconfirmation_start_l2_slot(&self, preconfer: bool, is_driver_synced: bool) -> bool {
        !self.was_synced_preconfer && preconfer && is_driver_synced
    }

    fn is_handover_window(&self, slot: Slot) -> bool {
        self.slot_clock
            .is_slot_in_last_n_slots_of_epoch(slot, self.handover_window_slots)
    }

    fn get_ms_from_handover_window_start(&self, l1_slot: Slot) -> Result<u64, Error> {
        let result: u64 = self
            .slot_clock
            .time_from_n_last_slots_of_epoch(l1_slot, self.handover_window_slots)?
            .as_millis()
            .try_into()
            .map_err(|err| {
                anyhow::anyhow!("is_handover_window: Field to covert u128 to u64: {:?}", err)
            })?;
        Ok(result)
    }

    async fn is_block_height_synced_between_taiko_geth_and_the_driver(
        &self,
        status: &TaikoStatus,
        l2_slot_info: &L2SlotInfo,
    ) -> Result<bool, Error> {
        if status.highest_unsafe_l2_payload_block_id == 0 {
            return Ok(true);
        }

        let taiko_geth_height = l2_slot_info.parent_id();
        if taiko_geth_height != status.highest_unsafe_l2_payload_block_id {
            warn!(
                "highestUnsafeL2PayloadBlockID: {}, different from Taiko Geth Height: {}",
                status.highest_unsafe_l2_payload_block_id, taiko_geth_height
            );
        }

        Ok(taiko_geth_height == status.highest_unsafe_l2_payload_block_id)
    }

    async fn is_taiko_geth_synced_with_l1(&self, l2_slot_info: &L2SlotInfo) -> Result<bool, Error> {
        let taiko_inbox_height = self
            .execution_layer
            .get_l2_height_from_taiko_inbox()
            .await?;

        Ok(l2_slot_info.parent_id() >= taiko_inbox_height)
    }

    async fn get_handover_window_slots(&self) -> u64 {
        match self.execution_layer.get_handover_window_slots().await {
            Ok(router_config) => router_config,
            Err(e) => {
                warn!(
                    "Failed to get preconf router config, using default handover window slots: {}",
                    e
                );
                self.handover_window_slots_default
            }
        }
    }
}
