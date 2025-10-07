pub fn is_next_fork_active(
    fork_timestamp: u64,
    handover_window_slots: u64,
    l1_slot_duration_sec: u64,
) -> bool {
    let current_timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    fork_timestamp != 0
        && current_timestamp >= fork_timestamp - (handover_window_slots * l1_slot_duration_sec)
}
