#[cfg(test)]
mod tests {
    use crate::node::operator::*;
    use alloy::primitives::B256;
    use chrono::DateTime;
    use common::l1::slot_clock::Clock;
    use common::l2::taiko_driver::models;
    use std::time::SystemTime;

    const HANDOVER_WINDOW_SLOTS: u64 = 6;

    #[derive(Default)]
    pub struct MockClock {
        pub timestamp: u64,
    }
    impl Clock for MockClock {
        fn now(&self) -> SystemTime {
            SystemTime::from(
                DateTime::from_timestamp(self.timestamp.try_into().unwrap(), 0).unwrap(),
            )
        }
    }

    struct ExecutionLayerMock {
        current_operator: bool,
        next_operator: bool,
        is_preconf_router_specified: bool,
        taiko_inbox_height: u64,
        handover_window_slots: u64,
    }

    impl PreconfOperator for ExecutionLayerMock {
        async fn is_operator_for_current_epoch(&self) -> Result<bool, Error> {
            Ok(self.current_operator)
        }

        async fn is_operator_for_next_epoch(&self) -> Result<bool, Error> {
            Ok(self.next_operator)
        }

        async fn is_preconf_router_specified_in_taiko_wrapper(&self) -> Result<bool, Error> {
            Ok(self.is_preconf_router_specified)
        }

        async fn get_l2_height_from_taiko_inbox(&self) -> Result<u64, Error> {
            Ok(self.taiko_inbox_height)
        }

        async fn get_handover_window_slots(&self) -> Result<u64, Error> {
            Ok(self.handover_window_slots)
        }
    }

    struct ExecutionLayerMockError {}
    impl PreconfOperator for ExecutionLayerMockError {
        async fn is_operator_for_current_epoch(&self) -> Result<bool, Error> {
            Err(Error::from(anyhow::anyhow!("test error")))
        }

        async fn is_operator_for_next_epoch(&self) -> Result<bool, Error> {
            Err(Error::from(anyhow::anyhow!("test error")))
        }

        async fn is_preconf_router_specified_in_taiko_wrapper(&self) -> Result<bool, Error> {
            Err(Error::from(anyhow::anyhow!("test error")))
        }

        async fn get_l2_height_from_taiko_inbox(&self) -> Result<u64, Error> {
            Err(Error::from(anyhow::anyhow!("test error")))
        }

        async fn get_handover_window_slots(&self) -> Result<u64, Error> {
            Err(Error::from(anyhow::anyhow!("test error")))
        }
    }

    struct TaikoUnsyncedMock {
        end_of_sequencing_block_hash: B256,
    }

    impl StatusProvider for TaikoUnsyncedMock {
        async fn get_status(&self) -> Result<models::TaikoStatus, Error> {
            Ok(models::TaikoStatus {
                end_of_sequencing_block_hash: self.end_of_sequencing_block_hash,
                highest_unsafe_l2_payload_block_id: 2,
            })
        }
    }

    struct TaikoMock {
        end_of_sequencing_block_hash: B256,
    }
    impl StatusProvider for TaikoMock {
        async fn get_status(&self) -> Result<models::TaikoStatus, Error> {
            Ok(models::TaikoStatus {
                end_of_sequencing_block_hash: self.end_of_sequencing_block_hash,
                highest_unsafe_l2_payload_block_id: 0,
            })
        }
    }

    fn get_l2_slot_info() -> L2SlotInfo {
        L2SlotInfo::new(
            0,
            0,
            0,
            B256::from([
                0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1,
                0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1,
            ]),
            0,
        )
    }

    #[tokio::test]
    async fn test_preconf_router_not_specified() {
        let mut operator = create_operator(
            32 * 12 + 2, // first l1 slot, second l2 slot
            true,
            false,
            false,
        );
        operator.next_operator = true;
        operator.was_synced_preconfer = true;
        operator.continuing_role = false;
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(false, false, false, false, false),
        );
    }

    #[tokio::test]
    async fn test_end_of_sequencing() {
        // End of sequencing
        let mut operator = create_operator(
            (31u64 - HANDOVER_WINDOW_SLOTS) * 12 + 5 * 2, // l1 slot before handover window, 5th l2 slot
            true,
            false,
            true,
        );
        operator.next_operator = false;
        operator.was_synced_preconfer = true;
        operator.continuing_role = false;
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(true, true, false, true, true)
        );
        // Not a preconfer and submiter
        let mut operator = create_operator(
            (31 - HANDOVER_WINDOW_SLOTS) * 12 + 5 * 2, // l1 slot before handover window, 5th l2 slot
            false,
            false,
            true,
        );
        operator.next_operator = false;
        operator.was_synced_preconfer = false;
        operator.continuing_role = false;
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(false, false, false, false, true)
        );
        // Continuing role
        let mut operator = create_operator(
            (31 - HANDOVER_WINDOW_SLOTS) * 12 + 5 * 2, // l1 slot before handover window, 5th l2 slot
            true,
            true,
            true,
        );
        operator.next_operator = true;
        operator.was_synced_preconfer = true;
        operator.continuing_role = true;
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(true, true, false, false, true)
        );
        // Not correct l2 slot
        let mut operator = create_operator(
            (31 - HANDOVER_WINDOW_SLOTS) * 12 + 4 * 2, // l1 slot before handover window, 4th l2 slot
            true,
            false,
            true,
        );
        operator.next_operator = false;
        operator.was_synced_preconfer = true;
        operator.continuing_role = false;
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(true, true, false, false, true)
        );
    }

    #[tokio::test]
    async fn test_get_preconfer_and_verifier_status() {
        let mut operator = create_operator(
            32 * 12 + 2, // first l1 slot, second l2 slot
            true,
            false,
            true,
        );
        operator.next_operator = true;
        operator.was_synced_preconfer = true;
        operator.continuing_role = false;
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(true, true, false, false, true)
        );

        let mut operator = create_operator(
            32 * 12 + 2, // first l1 slot, second l2 slot
            false,
            false,
            true,
        );
        operator.was_synced_preconfer = true;
        operator.continuing_role = true;
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(false, false, false, false, true)
        );
    }

    #[tokio::test]
    async fn test_get_second_slot_status() {
        let mut operator = create_operator(
            32 * 12 + 12 + 2, // second l1 slot, second l2 slot
            true,
            false,
            true,
        );
        operator.next_operator = true;
        operator.was_synced_preconfer = true;
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(true, true, false, false, true)
        );

        let mut operator = create_operator(
            32 * 12 + 12 + 2, // second l1 slot, second l2 slot
            false,
            false,
            true,
        );
        operator.was_synced_preconfer = true;
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(false, false, false, false, true)
        );
    }

    #[tokio::test]
    async fn test_is_driver_synced_status() {
        let mut operator = create_operator_with_unsynced_driver_and_geth(
            31 * 12, // last slot of epoch
            false,
            true,
            true,
        );
        operator.was_synced_preconfer = true;
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(true, false, false, false, false)
        );

        let mut operator = create_operator_with_high_taiko_inbox_height();
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(false, false, false, false, false)
        );
    }

    #[tokio::test]
    async fn test_get_preconfer_status() {
        let mut operator = create_operator(
            31 * 12, // last slot of epoch
            false,
            true,
            true,
        );
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(true, false, true, false, true)
        );

        let mut operator = create_operator(
            32 * 12, // first slot of next epoch
            true,
            false,
            true,
        );
        operator.next_operator = true;
        operator.was_synced_preconfer = true;
        operator.continuing_role = false;
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(true, true, false, false, true)
        );

        let mut operator = create_operator(
            32 * 12, // first slot of next epoch
            true,
            false,
            true,
        );
        operator.next_operator = true;
        operator.was_synced_preconfer = true;
        operator.continuing_role = true;
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(true, true, false, false, true)
        );
    }

    #[tokio::test]
    async fn test_get_none_status() {
        // Not an operator at all
        let mut operator = create_operator(
            20 * 12, // middle of epoch
            false,
            false,
            true,
        );
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(false, false, false, false, true)
        );

        // First slot of epoch, not nominated
        let mut operator = create_operator(
            32 * 12, // first slot of next epoch
            false,
            false,
            true,
        );
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(false, false, false, false, true)
        );

        let mut operator = create_operator(
            31 * 12, // last slot
            false,
            false,
            true,
        );
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(false, false, false, false, true)
        );
    }

    #[tokio::test]
    async fn test_get_preconfer_handover_buffer_status() {
        // Next operator in handover window, but still in buffer period
        let mut operator = create_operator(
            (32 - HANDOVER_WINDOW_SLOTS) * 12, // handover buffer
            false,
            true,
            true,
        );
        // Override the handover start buffer to be larger than the mock timestamp
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(false, false, false, false, true)
        );

        let mut operator = create_operator(
            (32 - HANDOVER_WINDOW_SLOTS + 1) * 12, // handover window after the buffer
            false,
            true,
            true,
        );
        // Override the handover start buffer to be larger than the mock timestamp
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(true, false, true, false, true)
        );
    }

    #[tokio::test]
    async fn test_get_preconfer_handover_buffer_status_with_end_of_sequencing_marker_received() {
        // Next operator in handover window, but still in buffer period
        let mut operator = create_operator_with_end_of_sequencing_marker_received(
            (32 - HANDOVER_WINDOW_SLOTS) * 12, // handover buffer
            false,
            true,
            true,
        );
        // Override the handover start buffer to be larger than the mock timestamp
        assert_eq!(
            operator
                .get_status(&L2SlotInfo::new(0, 0, 0, get_test_hash(), 0))
                .await
                .unwrap(),
            Status::new(true, false, true, false, true)
        );
    }

    #[tokio::test]
    async fn test_get_preconfer_and_l1_submitter_status() {
        // Current operator and next operator (continuing role)
        let mut operator = create_operator(
            31 * 12, // last slot of epoch (handover window)
            true,
            true,
            true,
        );
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(true, true, true, false, true)
        );

        // Current operator outside handover window
        let mut operator = create_operator(
            20 * 12, // middle of epoch
            true,
            false,
            true,
        );
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(true, true, true, false, true)
        );
    }

    #[tokio::test]
    async fn test_long_handover_window_from_config() {
        let mut operator = create_operator_with_long_handover_window_from_config();
        assert_eq!(operator.handover_window_slots, HANDOVER_WINDOW_SLOTS);
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(false, true, false, false, true)
        );

        // during get_status, new handover window slots should be loaded from config
        assert_eq!(operator.handover_window_slots, 10);

        // another get_status call should not change the handover window slots
        operator.get_status(&get_l2_slot_info()).await.unwrap();
        assert_eq!(operator.handover_window_slots, 10);
    }

    #[tokio::test]
    async fn test_get_status_with_error_in_execution_layer() {
        let operator = create_operator_with_error_in_execution_layer();
        assert_eq!(
            operator.get_handover_window_slots().await,
            HANDOVER_WINDOW_SLOTS
        );
    }

    #[tokio::test]
    async fn test_get_l1_submitter_status() {
        // Current operator but not next operator during handover window
        let mut operator = create_operator(
            31 * 12, // last slot of epoch
            true,
            false,
            true,
        );
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(false, true, false, false, true)
        );
    }

    #[tokio::test]
    async fn test_get_l1_statuses_for_operator_continuing_role() {
        let mut operator = create_operator(
            0, // first slot of epoch
            true, true, true,
        );
        operator.next_operator = true;
        operator.continuing_role = true;
        operator.was_synced_preconfer = true;

        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(true, true, false, false, true)
        );

        let mut operator = create_operator(
            12, // second slot of epoch
            true, true, true,
        );
        operator.next_operator = true;
        operator.continuing_role = true;
        operator.was_synced_preconfer = true;
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(true, true, false, false, true)
        );

        let mut operator = create_operator(
            2 * 12, // third slot of epoch
            true,
            true,
            true,
        );
        operator.continuing_role = true;
        operator.was_synced_preconfer = true;
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(true, true, false, false, true)
        );
    }

    #[tokio::test]
    async fn test_get_preconfirmation_started_status() {
        let mut operator = create_operator(
            31 * 12, // last slot of epoch
            false,
            true,
            true,
        );
        operator.was_synced_preconfer = false;
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(true, false, true, false, true)
        );

        // second get_status call, preconfirmation_started should be false
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(true, false, false, false, true)
        );
    }

    fn create_operator(
        timestamp: u64,
        current_operator: bool,
        next_operator: bool,
        is_preconf_router_specified: bool,
    ) -> Operator<ExecutionLayerMock, MockClock, TaikoMock> {
        let mut slot_clock = SlotClock::<MockClock>::new(0, 0, 12, 32, 2000);
        slot_clock.clock.timestamp = timestamp;
        Operator {
            cancel_token: CancellationToken::new(),
            last_config_reload_epoch: 0,
            cancel_counter: 0,
            taiko: Arc::new(TaikoMock {
                end_of_sequencing_block_hash: B256::ZERO,
            }),
            execution_layer: Arc::new(ExecutionLayerMock {
                current_operator,
                next_operator,
                is_preconf_router_specified,
                taiko_inbox_height: 0,
                handover_window_slots: HANDOVER_WINDOW_SLOTS,
            }),
            slot_clock: Arc::new(slot_clock),
            handover_window_slots: HANDOVER_WINDOW_SLOTS,
            handover_window_slots_default: HANDOVER_WINDOW_SLOTS,
            handover_start_buffer_ms: 1000,
            next_operator: false,
            continuing_role: false,
            simulate_not_submitting_at_the_end_of_epoch: false,
            was_synced_preconfer: false,
            operator_transition_slots: 1,
        }
    }

    fn create_operator_with_end_of_sequencing_marker_received(
        timestamp: u64,
        current_operator: bool,
        next_operator: bool,
        is_preconf_router_specified: bool,
    ) -> Operator<ExecutionLayerMock, MockClock, TaikoMock> {
        let mut slot_clock = SlotClock::<MockClock>::new(0, 0, 12, 32, 2000);
        slot_clock.clock.timestamp = timestamp;
        Operator {
            cancel_token: CancellationToken::new(),
            last_config_reload_epoch: 0,
            taiko: Arc::new(TaikoMock {
                end_of_sequencing_block_hash: get_test_hash(),
            }),
            execution_layer: Arc::new(ExecutionLayerMock {
                current_operator,
                next_operator,
                is_preconf_router_specified,
                taiko_inbox_height: 0,
                handover_window_slots: HANDOVER_WINDOW_SLOTS,
            }),
            slot_clock: Arc::new(slot_clock),
            handover_window_slots: HANDOVER_WINDOW_SLOTS,
            handover_window_slots_default: HANDOVER_WINDOW_SLOTS,
            handover_start_buffer_ms: 1000,
            next_operator: false,
            continuing_role: false,
            simulate_not_submitting_at_the_end_of_epoch: false,
            was_synced_preconfer: false,
            cancel_counter: 0,
            operator_transition_slots: 1,
        }
    }

    fn create_operator_with_unsynced_driver_and_geth(
        timestamp: u64,
        current_operator: bool,
        next_operator: bool,
        is_preconf_router_specified: bool,
    ) -> Operator<ExecutionLayerMock, MockClock, TaikoUnsyncedMock> {
        let mut slot_clock = SlotClock::<MockClock>::new(0, 0, 12, 32, 2000);
        slot_clock.clock.timestamp = timestamp;
        Operator {
            cancel_token: CancellationToken::new(),
            last_config_reload_epoch: 0,
            taiko: Arc::new(TaikoUnsyncedMock {
                end_of_sequencing_block_hash: get_test_hash(),
            }),
            execution_layer: Arc::new(ExecutionLayerMock {
                current_operator,
                next_operator,
                is_preconf_router_specified,
                taiko_inbox_height: 0,
                handover_window_slots: HANDOVER_WINDOW_SLOTS,
            }),
            slot_clock: Arc::new(slot_clock),
            handover_window_slots: HANDOVER_WINDOW_SLOTS,
            handover_window_slots_default: HANDOVER_WINDOW_SLOTS,
            handover_start_buffer_ms: 1000,
            next_operator: false,
            continuing_role: false,
            simulate_not_submitting_at_the_end_of_epoch: false,
            was_synced_preconfer: false,
            cancel_counter: 0,
            operator_transition_slots: 1,
        }
    }

    fn create_operator_with_high_taiko_inbox_height()
    -> Operator<ExecutionLayerMock, MockClock, TaikoMock> {
        let slot_clock = SlotClock::<MockClock>::new(0, 0, 12, 32, 2000);
        Operator {
            cancel_token: CancellationToken::new(),
            last_config_reload_epoch: 0,
            cancel_counter: 0,
            taiko: Arc::new(TaikoMock {
                end_of_sequencing_block_hash: B256::ZERO,
            }),
            execution_layer: Arc::new(ExecutionLayerMock {
                current_operator: true,
                next_operator: true,
                is_preconf_router_specified: true,
                taiko_inbox_height: 1000,
                handover_window_slots: HANDOVER_WINDOW_SLOTS,
            }),
            slot_clock: Arc::new(slot_clock),
            handover_window_slots: HANDOVER_WINDOW_SLOTS,
            handover_window_slots_default: HANDOVER_WINDOW_SLOTS,
            handover_start_buffer_ms: 1000,
            next_operator: false,
            continuing_role: false,
            simulate_not_submitting_at_the_end_of_epoch: false,
            was_synced_preconfer: false,
            operator_transition_slots: 1,
        }
    }

    fn create_operator_with_long_handover_window_from_config()
    -> Operator<ExecutionLayerMock, MockClock, TaikoMock> {
        let mut slot_clock = SlotClock::<MockClock>::new(0, 0, 12, 32, 2000);
        slot_clock.clock.timestamp = 32 * 12 + 25 * 12; // second epoch 26th slot
        Operator {
            cancel_token: CancellationToken::new(),
            last_config_reload_epoch: 0,
            cancel_counter: 0,
            taiko: Arc::new(TaikoMock {
                end_of_sequencing_block_hash: B256::ZERO,
            }),
            execution_layer: Arc::new(ExecutionLayerMock {
                current_operator: true,
                next_operator: false,
                is_preconf_router_specified: true,
                taiko_inbox_height: 0,
                handover_window_slots: 10,
            }),
            slot_clock: Arc::new(slot_clock),
            handover_window_slots: HANDOVER_WINDOW_SLOTS,
            handover_window_slots_default: HANDOVER_WINDOW_SLOTS,
            handover_start_buffer_ms: 1000,
            next_operator: false,
            continuing_role: false,
            simulate_not_submitting_at_the_end_of_epoch: false,
            was_synced_preconfer: false,
            operator_transition_slots: 1,
        }
    }

    fn create_operator_with_error_in_execution_layer()
    -> Operator<ExecutionLayerMockError, MockClock, TaikoMock> {
        let slot_clock = SlotClock::<MockClock>::new(0, 0, 12, 32, 2000);
        Operator {
            cancel_token: CancellationToken::new(),
            last_config_reload_epoch: 0,
            cancel_counter: 0,
            taiko: Arc::new(TaikoMock {
                end_of_sequencing_block_hash: B256::ZERO,
            }),
            execution_layer: Arc::new(ExecutionLayerMockError {}),
            slot_clock: Arc::new(slot_clock),
            handover_window_slots: HANDOVER_WINDOW_SLOTS,
            handover_window_slots_default: HANDOVER_WINDOW_SLOTS,
            handover_start_buffer_ms: 1000,
            next_operator: false,
            continuing_role: false,
            simulate_not_submitting_at_the_end_of_epoch: false,
            was_synced_preconfer: false,
            operator_transition_slots: 1,
        }
    }

    fn get_test_hash() -> B256 {
        B256::from([
            0x12, 0x34, 0x56, 0x78, 0x90, 0xab, 0xcd, 0xef, 0x12, 0x34, 0x56, 0x78, 0x90, 0xab,
            0xcd, 0xef, 0x12, 0x34, 0x56, 0x78, 0x90, 0xab, 0xcd, 0xef, 0x12, 0x34, 0x56, 0x78,
            0x90, 0xab, 0xcd, 0xef,
        ])
    }
}
