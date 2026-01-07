#[cfg(test)]
mod tests {
    use crate::l1::traits::OperatorError;
    use crate::node::operator::*;
    use alloy::primitives::B256;
    use alloy::primitives::{Address, address};
    use chrono::DateTime;
    use common::shared::l2_slot_info::L2SlotInfo;
    use common::{l1::slot_clock::Clock, l2::taiko_driver::models, metrics::Metrics};
    use std::time::SystemTime;

    const HANDOVER_WINDOW_SLOTS: u64 = 6;
    const PRECONFER_ADDRESS: Address = address!("0x1234567890123456789012345678901234567890");
    const OTHER_OPERATOR_ADDRESS: Address = address!("0x1234567890123456789012345678901234567891");

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
        current_operator_address: Address,
        next_operator_address: Address,
        is_preconf_router_specified: bool,
        taiko_inbox_height: u64,
        handover_window_slots: u64,
    }

    impl PreconfOperator for ExecutionLayerMock {
        fn get_preconfer_address(&self) -> Address {
            PRECONFER_ADDRESS
        }

        async fn get_operators_for_current_and_next_epoch(
            &self,
            _: u64,
        ) -> Result<(Address, Address), OperatorError> {
            Ok((self.current_operator_address, self.next_operator_address))
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
        fn get_preconfer_address(&self) -> Address {
            PRECONFER_ADDRESS
        }

        async fn get_operators_for_current_and_next_epoch(
            &self,
            _: u64,
        ) -> Result<(Address, Address), OperatorError> {
            Err(OperatorError::Any(Error::from(anyhow::anyhow!(
                "test error"
            ))))
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
            Status::new(
                false,
                false,
                false,
                false,
                false,
                #[cfg(feature = "get_status_duration")]
                None,
            ),
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
            Status::new(
                true,
                true,
                false,
                true,
                true,
                #[cfg(feature = "get_status_duration")]
                None,
            )
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
            Status::new(
                false,
                false,
                false,
                false,
                true,
                #[cfg(feature = "get_status_duration")]
                None,
            )
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
            Status::new(
                true,
                true,
                false,
                false,
                true,
                #[cfg(feature = "get_status_duration")]
                None,
            )
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
            Status::new(
                true,
                true,
                false,
                false,
                true,
                #[cfg(feature = "get_status_duration")]
                None,
            )
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
            Status::new(
                true,
                true,
                false,
                false,
                true,
                #[cfg(feature = "get_status_duration")]
                None,
            )
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
            Status::new(
                false,
                false,
                false,
                false,
                true,
                #[cfg(feature = "get_status_duration")]
                None,
            )
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
            Status::new(
                true,
                true,
                false,
                false,
                true,
                #[cfg(feature = "get_status_duration")]
                None,
            )
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
            Status::new(
                false,
                false,
                false,
                false,
                true,
                #[cfg(feature = "get_status_duration")]
                None,
            )
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
            Status::new(
                true,
                false,
                false,
                false,
                false,
                #[cfg(feature = "get_status_duration")]
                None,
            )
        );

        let mut operator = create_operator_with_high_taiko_inbox_height();
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(
                true,
                true,
                false,
                false,
                false,
                #[cfg(feature = "get_status_duration")]
                None,
            )
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
            Status::new(
                true,
                false,
                true,
                false,
                true,
                #[cfg(feature = "get_status_duration")]
                None,
            )
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
            Status::new(
                true,
                true,
                false,
                false,
                true,
                #[cfg(feature = "get_status_duration")]
                None,
            )
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
            Status::new(
                true,
                true,
                false,
                false,
                true,
                #[cfg(feature = "get_status_duration")]
                None,
            )
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
            Status::new(
                false,
                false,
                false,
                false,
                true,
                #[cfg(feature = "get_status_duration")]
                None,
            )
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
            Status::new(
                false,
                false,
                false,
                false,
                true,
                #[cfg(feature = "get_status_duration")]
                None,
            )
        );

        let mut operator = create_operator(
            31 * 12, // last slot
            false,
            false,
            true,
        );
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(
                false,
                false,
                false,
                false,
                true,
                #[cfg(feature = "get_status_duration")]
                None,
            )
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
            Status::new(
                false,
                false,
                false,
                false,
                true,
                #[cfg(feature = "get_status_duration")]
                None,
            )
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
            Status::new(
                true,
                false,
                true,
                false,
                true,
                #[cfg(feature = "get_status_duration")]
                None,
            )
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
                .get_status(&L2SlotInfo::new(0, 0, 0, get_test_hash(), 0, 0))
                .await
                .unwrap(),
            Status::new(
                true,
                false,
                true,
                false,
                true,
                #[cfg(feature = "get_status_duration")]
                None,
            )
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
            Status::new(
                true,
                true,
                true,
                false,
                true,
                #[cfg(feature = "get_status_duration")]
                None,
            )
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
            Status::new(
                true,
                true,
                true,
                false,
                true,
                #[cfg(feature = "get_status_duration")]
                None,
            )
        );
    }

    #[tokio::test]
    async fn test_long_handover_window_from_config() {
        let mut operator = create_operator_with_long_handover_window_from_config();
        assert_eq!(operator.handover_window_slots, HANDOVER_WINDOW_SLOTS);
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(
                false,
                true,
                false,
                false,
                true,
                #[cfg(feature = "get_status_duration")]
                None,
            )
        );

        // during get_status, new handover window slots should be loaded from config
        assert_eq!(operator.handover_window_slots, 10);

        // another get_status call should not change the handover window slots
        operator.get_status(&get_l2_slot_info()).await.unwrap();
        assert_eq!(operator.handover_window_slots, 10);
    }

    #[tokio::test]
    async fn test_get_status_with_error_in_execution_layer() {
        let operator =
            create_operator_with_error_in_execution_layer(Arc::new(ExecutionLayerMockError {}));
        assert_eq!(
            operator.get_handover_window_slots().await,
            HANDOVER_WINDOW_SLOTS
        );
    }

    struct ExecutionLayerMockErrorToEarly {}
    impl PreconfOperator for ExecutionLayerMockErrorToEarly {
        fn get_preconfer_address(&self) -> Address {
            PRECONFER_ADDRESS
        }

        async fn get_operators_for_current_and_next_epoch(
            &self,
            _: u64,
        ) -> Result<(Address, Address), OperatorError> {
            Err(OperatorError::OperatorCheckTooEarly)
        }

        async fn is_preconf_router_specified_in_taiko_wrapper(&self) -> Result<bool, Error> {
            Ok(true)
        }

        async fn get_l2_height_from_taiko_inbox(&self) -> Result<u64, Error> {
            Ok(0)
        }

        async fn get_handover_window_slots(&self) -> Result<u64, Error> {
            Ok(HANDOVER_WINDOW_SLOTS)
        }
    }

    #[tokio::test]
    async fn test_get_status_with_error_too_early_in_execution_layer() {
        let mut operator = create_operator_with_error_in_execution_layer(Arc::new(
            ExecutionLayerMockErrorToEarly {},
        ));

        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(
                true,
                true,
                true,
                false,
                true,
                #[cfg(feature = "get_status_duration")]
                None,
            )
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
            Status::new(
                false,
                true,
                false,
                false,
                true,
                #[cfg(feature = "get_status_duration")]
                None,
            )
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
            Status::new(
                true,
                true,
                false,
                false,
                true,
                #[cfg(feature = "get_status_duration")]
                None,
            )
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
            Status::new(
                true,
                true,
                false,
                false,
                true,
                #[cfg(feature = "get_status_duration")]
                None,
            )
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
            Status::new(
                true,
                true,
                false,
                false,
                true,
                #[cfg(feature = "get_status_duration")]
                None,
            )
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
            Status::new(
                true,
                false,
                true,
                false,
                true,
                #[cfg(feature = "get_status_duration")]
                None,
            )
        );

        // second get_status call, preconfirmation_started should be false
        assert_eq!(
            operator.get_status(&get_l2_slot_info()).await.unwrap(),
            Status::new(
                true,
                false,
                false,
                false,
                true,
                #[cfg(feature = "get_status_duration")]
                None,
            )
        );
    }

    #[tokio::test]
    async fn test_get_status_with_fork_switch_transition_period() {
        // fork switch timestamp is 100 seconds
        const CURRENT_TIMESTAMP: u64 = 90;
        let mut operator = create_operator_with_fork_switch_transition_period(CURRENT_TIMESTAMP);
        let l2_slot_info = L2SlotInfo::new(0, CURRENT_TIMESTAMP, 0, get_test_hash(), 0, 0);
        assert_eq!(
            operator.get_status(&l2_slot_info).await.unwrap(),
            Status::new(
                false,
                true,
                false,
                false,
                true,
                #[cfg(feature = "get_status_duration")]
                None,
            )
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
        let (current_operator_address, next_operator_address) =
            get_operators(current_operator, next_operator);
        Operator {
            fork_info: ForkInfo::default(),
            cancel_token: CancellationToken::new(Arc::new(Metrics::new())),
            last_config_reload_epoch: 0,
            cancel_counter: 0,
            taiko: Arc::new(TaikoMock {
                end_of_sequencing_block_hash: B256::ZERO,
            }),
            execution_layer: Arc::new(ExecutionLayerMock {
                current_operator_address,
                next_operator_address,
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
            current_operator_address: Address::ZERO,
        }
    }

    fn get_operators(current_operator: bool, next_operator: bool) -> (Address, Address) {
        let current_operator_address = if current_operator {
            PRECONFER_ADDRESS
        } else {
            OTHER_OPERATOR_ADDRESS
        };
        let next_operator_address = if next_operator {
            PRECONFER_ADDRESS
        } else {
            OTHER_OPERATOR_ADDRESS
        };
        (current_operator_address, next_operator_address)
    }

    fn create_operator_with_end_of_sequencing_marker_received(
        timestamp: u64,
        current_operator: bool,
        next_operator: bool,
        is_preconf_router_specified: bool,
    ) -> Operator<ExecutionLayerMock, MockClock, TaikoMock> {
        let mut slot_clock = SlotClock::<MockClock>::new(0, 0, 12, 32, 2000);
        slot_clock.clock.timestamp = timestamp;
        let (current_operator_address, next_operator_address) =
            get_operators(current_operator, next_operator);
        Operator {
            fork_info: ForkInfo::default(),
            cancel_token: CancellationToken::new(Arc::new(Metrics::new())),
            last_config_reload_epoch: 0,
            taiko: Arc::new(TaikoMock {
                end_of_sequencing_block_hash: get_test_hash(),
            }),
            execution_layer: Arc::new(ExecutionLayerMock {
                current_operator_address,
                next_operator_address,
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
            current_operator_address: Address::ZERO,
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
        let (current_operator_address, next_operator_address) =
            get_operators(current_operator, next_operator);
        Operator {
            fork_info: ForkInfo::default(),
            cancel_token: CancellationToken::new(Arc::new(Metrics::new())),
            last_config_reload_epoch: 0,
            taiko: Arc::new(TaikoUnsyncedMock {
                end_of_sequencing_block_hash: get_test_hash(),
            }),
            execution_layer: Arc::new(ExecutionLayerMock {
                current_operator_address,
                next_operator_address,
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
            current_operator_address: Address::ZERO,
        }
    }

    fn create_operator_with_high_taiko_inbox_height()
    -> Operator<ExecutionLayerMock, MockClock, TaikoMock> {
        let slot_clock = SlotClock::<MockClock>::new(0, 0, 12, 32, 2000);
        Operator {
            fork_info: ForkInfo::default(),
            cancel_token: CancellationToken::new(Arc::new(Metrics::new())),
            last_config_reload_epoch: 0,
            cancel_counter: 0,
            taiko: Arc::new(TaikoMock {
                end_of_sequencing_block_hash: B256::ZERO,
            }),
            execution_layer: Arc::new(ExecutionLayerMock {
                current_operator_address: PRECONFER_ADDRESS,
                next_operator_address: PRECONFER_ADDRESS,
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
            current_operator_address: Address::ZERO,
        }
    }

    fn create_operator_with_long_handover_window_from_config()
    -> Operator<ExecutionLayerMock, MockClock, TaikoMock> {
        let mut slot_clock = SlotClock::<MockClock>::new(0, 0, 12, 32, 2000);
        slot_clock.clock.timestamp = 32 * 12 + 25 * 12; // second epoch 26th slot
        Operator {
            fork_info: ForkInfo::default(),
            cancel_token: CancellationToken::new(Arc::new(Metrics::new())),
            last_config_reload_epoch: 0,
            cancel_counter: 0,
            taiko: Arc::new(TaikoMock {
                end_of_sequencing_block_hash: B256::ZERO,
            }),
            execution_layer: Arc::new(ExecutionLayerMock {
                current_operator_address: PRECONFER_ADDRESS,
                next_operator_address: OTHER_OPERATOR_ADDRESS,
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
            current_operator_address: Address::ZERO,
        }
    }

    fn create_operator_with_error_in_execution_layer<T: PreconfOperator>(
        execution_layer: Arc<T>,
    ) -> Operator<T, MockClock, TaikoMock> {
        let slot_clock = SlotClock::<MockClock>::new(0, 0, 12, 32, 2000);
        Operator {
            fork_info: ForkInfo::default(),
            cancel_token: CancellationToken::new(Arc::new(Metrics::new())),
            last_config_reload_epoch: 0,
            cancel_counter: 0,
            taiko: Arc::new(TaikoMock {
                end_of_sequencing_block_hash: B256::ZERO,
            }),
            execution_layer,
            slot_clock: Arc::new(slot_clock),
            handover_window_slots: HANDOVER_WINDOW_SLOTS,
            handover_window_slots_default: HANDOVER_WINDOW_SLOTS,
            handover_start_buffer_ms: 1000,
            next_operator: true,
            continuing_role: false,
            simulate_not_submitting_at_the_end_of_epoch: false,
            was_synced_preconfer: false,
            current_operator_address: Address::ZERO,
        }
    }

    fn create_operator_with_fork_switch_transition_period(
        current_timestamp: u64,
    ) -> Operator<ExecutionLayerMock, MockClock, TaikoMock> {
        use common::fork_info::{ForkInfo, config::ForkInfoConfig, fork::Fork};
        use std::time::Duration;

        let mut slot_clock = SlotClock::<MockClock>::new(0, 0, 12, 32, 2000);
        slot_clock.clock.timestamp = current_timestamp;
        Operator {
            fork_info: ForkInfo {
                fork: Fork::Pacaya,
                config: ForkInfoConfig {
                    fork_switch_timestamps: vec![
                        Duration::from_secs(0),   // Pacaya
                        Duration::from_secs(100), // Shasta
                    ],
                    fork_switch_transition_period: Duration::from_secs(15),
                },
            },
            cancel_token: CancellationToken::new(Arc::new(Metrics::new())),
            last_config_reload_epoch: 0,
            cancel_counter: 0,
            taiko: Arc::new(TaikoMock {
                end_of_sequencing_block_hash: B256::ZERO,
            }),
            execution_layer: Arc::new(ExecutionLayerMock {
                current_operator_address: PRECONFER_ADDRESS,
                next_operator_address: PRECONFER_ADDRESS,
                is_preconf_router_specified: true,
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
            current_operator_address: Address::ZERO,
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
