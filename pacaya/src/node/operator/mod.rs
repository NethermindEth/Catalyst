mod status;
mod tests;

use crate::l1::{OperatorError, PreconfOperator};
use alloy::primitives::Address;
use anyhow::Error;
use common::{
    fork_info::ForkInfo,
    l1::slot_clock::{Clock, SlotClock},
    l2::taiko_driver::{StatusProvider, models::TaikoStatus},
    shared::l2_slot_info::SlotData,
    utils::{cancellation_token::CancellationToken, types::*},
};
pub use status::Status;
use std::sync::Arc;
use tracing::{debug, info, warn};

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
    last_config_reload_epoch: u64,
    fork_info: ForkInfo,
    current_operator_address: Address,
}

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
            last_config_reload_epoch: 0,
            fork_info,
            current_operator_address: Address::ZERO,
        })
    }

    /// Get the current status of the operator based on the current L1 and L2 slots
    pub async fn get_status<S: SlotData>(&mut self, l2_slot_info: &S) -> Result<Status, Error> {
        // feature get_status_duration
        #[cfg(feature = "get_status_duration")]
        let start = std::time::Instant::now();
        if !self
            .execution_layer
            .is_preconf_router_specified_in_taiko_wrapper()
            .await?
        {
            warn!("PreconfRouter is not specified in TaikoWrapper");
            self.reset();
            return Ok(Status::new(
                false,
                false,
                false,
                false,
                false,
                #[cfg(feature = "get_status_duration")]
                None,
            ));
        }
        #[cfg(feature = "get_status_duration")]
        let check_taiko_wrapper = start.elapsed();

        let l1_slot: u64 = self.slot_clock.get_current_slot_of_epoch()?;

        let epoch = self.slot_clock.get_current_epoch()?;
        if epoch > self.last_config_reload_epoch {
            self.handover_window_slots = self.get_handover_window_slots().await;
            debug!(
                "Reloaded router config. Handover window slots: {}",
                self.handover_window_slots
            );
            self.last_config_reload_epoch = epoch;
        }
        #[cfg(feature = "get_status_duration")]
        let check_handover_window_slots = start.elapsed();

        let current_operator = self.is_current_operator(epoch).await?;
        #[cfg(feature = "get_status_duration")]
        let check_current_operator = start.elapsed();
        let handover_window = self.is_handover_window(l1_slot);
        #[cfg(feature = "get_status_duration")]
        let check_handover_window = start.elapsed();
        let driver_status = self.taiko.get_status().await?;
        #[cfg(feature = "get_status_duration")]
        let check_driver_status = start.elapsed();
        let is_driver_synced = self.is_driver_synced(l2_slot_info, &driver_status).await?;
        #[cfg(feature = "get_status_duration")]
        let check_driver_synced = start.elapsed();
        let preconfer = self
            .is_preconfer(
                current_operator,
                handover_window,
                l1_slot,
                l2_slot_info,
                &driver_status,
            )
            .await?;
        #[cfg(feature = "get_status_duration")]
        let check_preconfer = start.elapsed();
        let preconfirmation_started =
            self.is_preconfirmation_start_l2_slot(preconfer, is_driver_synced);
        #[cfg(feature = "get_status_duration")]
        let check_preconfirmation_started = start.elapsed();
        if preconfirmation_started {
            self.was_synced_preconfer = true;
        }
        if !preconfer {
            self.was_synced_preconfer = false;
        }

        let submitter = self.is_submitter(current_operator, handover_window);
        #[cfg(feature = "get_status_duration")]
        let check_submitter = start.elapsed();
        let end_of_sequencing = self.is_end_of_sequencing(preconfer, submitter, l1_slot)?;
        #[cfg(feature = "get_status_duration")]
        let check_end_of_sequencing = start.elapsed();

        #[cfg(feature = "get_status_duration")]
        let durations = status::StatusCheckDurations {
            check_taiko_wrapper,
            check_handover_window_slots,
            check_current_operator,
            check_handover_window,
            check_driver_status,
            check_driver_synced,
            check_preconfer,
            check_preconfirmation_started,
            check_submitter,
            check_end_of_sequencing,
        };
        Ok(Status::new(
            preconfer,
            submitter,
            preconfirmation_started,
            end_of_sequencing,
            is_driver_synced,
            #[cfg(feature = "get_status_duration")]
            Some(durations),
        ))
    }

    async fn is_current_operator(&mut self, epoch: u64) -> Result<bool, Error> {
        let current_epoch_timestamp = self.slot_clock.get_epoch_begin_timestamp(epoch)?;
        match self
            .execution_layer
            .get_operators_for_current_and_next_epoch(current_epoch_timestamp)
            .await
        {
            Ok((current_operator_address, next_operator_address)) => {
                if current_operator_address != self.current_operator_address {
                    info!(
                        "Operator has changed from {} to {}. Next operator: {}",
                        self.current_operator_address,
                        current_operator_address,
                        next_operator_address
                    );
                    self.current_operator_address = current_operator_address;
                }
                let current_operator =
                    current_operator_address == self.execution_layer.get_preconfer_address();
                self.next_operator =
                    next_operator_address == self.execution_layer.get_preconfer_address();
                self.continuing_role = current_operator && self.next_operator;
                Ok(current_operator)
            }
            Err(OperatorError::OperatorCheckTooEarly) => {
                debug!("Operator check too early, using next operator");
                Ok(self.next_operator)
            }
            Err(OperatorError::Any(e)) => Err(Error::msg(format!(
                "Failed to check current epoch operator: {e}"
            ))),
        }
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

    async fn is_driver_synced<S: SlotData>(
        &mut self,
        l2_slot_info: &S,
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

    async fn is_preconfer<S: SlotData>(
        &mut self,
        current_operator: bool,
        handover_window: bool,
        l1_slot: Slot,
        l2_slot_info: &S,
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

    async fn is_handover_buffer<S: SlotData>(
        &self,
        l1_slot: Slot,
        l2_slot_info: &S,
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

    fn end_of_sequencing_marker_received<S: SlotData>(
        &self,
        driver_status: &TaikoStatus,
        l2_slot_info: &S,
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
                anyhow::anyhow!(
                    "is_handover_window: Failed to convert u128 to u64: {:?}",
                    err
                )
            })?;
        Ok(result)
    }

    async fn is_block_height_synced_between_taiko_geth_and_the_driver<S: SlotData>(
        &self,
        status: &TaikoStatus,
        l2_slot_info: &S,
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

    async fn is_taiko_geth_synced_with_l1<S: SlotData>(
        &self,
        l2_slot_info: &S,
    ) -> Result<bool, Error> {
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
