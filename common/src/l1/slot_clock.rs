#![allow(unused)]

use crate::utils::types::*;
use anyhow::Error;
use std::{
    thread::current,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub trait Clock: Default {
    fn now(&self) -> SystemTime;
}

#[derive(Default)]
pub struct RealClock;
impl Clock for RealClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// Determines the present slot based upon a manually-incremented UNIX timestamp.
/// based on: https://github.com/sigp/lighthouse/blob/stable/common/slot_clock/src/manual_slot_clock.rs
pub struct SlotClock<T: Clock = RealClock> {
    genesis_slot: Slot,
    /// Duration from UNIX epoch to genesis.
    genesis_duration: Duration,
    /// The length of each slot.
    slot_duration: Duration,
    slots_per_epoch: u64,
    l2_slots_per_l1: u64,
    preconf_heartbeat_ms: u64,
    pub clock: T,
}

impl<T: Clock> SlotClock<T> {
    pub fn new(
        genesis_slot: Slot,
        genesis_timestamp_sec: u64,
        slot_duration_sec: u64,
        slots_per_epoch: u64,
        preconf_heartbeat_ms: u64,
    ) -> Self {
        tracing::info!(
            "SlotClock: genesis_timestamp_sec: {}, genesis_slot: {}",
            genesis_timestamp_sec,
            genesis_slot
        );

        let slot_duration = Duration::from_secs(slot_duration_sec);
        let l2_slots_per_l1 = slot_duration_sec * 1000 / preconf_heartbeat_ms;
        Self {
            genesis_slot,
            genesis_duration: Duration::from_secs(genesis_timestamp_sec),
            slot_duration,
            slots_per_epoch,
            l2_slots_per_l1,
            preconf_heartbeat_ms,
            clock: T::default(),
        }
    }

    pub fn get_number_of_l2_slots_per_l1(&self) -> u64 {
        self.l2_slots_per_l1
    }

    pub fn get_slot_duration(&self) -> Duration {
        self.slot_duration
    }

    pub fn get_epoch_duration(&self) -> Duration {
        Duration::from_secs(self.slot_duration.as_secs() * self.slots_per_epoch)
    }

    pub fn get_slots_per_epoch(&self) -> u64 {
        self.slots_per_epoch
    }

    pub fn duration_to_next_slot(&self) -> Result<Duration, Error> {
        let now = self.clock.now().duration_since(UNIX_EPOCH)?;
        self.duration_to_next_slot_from(now)
    }

    pub fn get_current_slot(&self) -> Result<Slot, Error> {
        let now = self.clock.now().duration_since(UNIX_EPOCH)?;
        self.slot_of(now)
    }

    pub fn get_preconf_heartbeat_ms(&self) -> u64 {
        self.preconf_heartbeat_ms
    }

    /// Returns the duration between `now` and the start of the next slot.
    pub fn duration_to_next_slot_from(&self, now: Duration) -> Result<Duration, Error> {
        if now < self.genesis_duration {
            Ok(self
                .genesis_duration
                .checked_sub(now)
                .ok_or(anyhow::anyhow!(
                    "duration_to_next_slot_from: Subtraction overflow"
                ))?)
        } else {
            self.duration_to_slot(self.slot_of(now)? + 1, now)
        }
    }

    pub fn duration_to_slot_from_now(&self, slot: Slot) -> Result<Duration, Error> {
        let now = self.clock.now().duration_since(UNIX_EPOCH)?;
        self.duration_to_slot(slot, now)
    }

    pub fn slot_of(&self, now: Duration) -> Result<Slot, Error> {
        let genesis: Duration = self.genesis_duration;

        if now >= genesis {
            let since_genesis = now
                .checked_sub(genesis)
                .ok_or(anyhow::anyhow!("slot_of: Subtraction overflow"))?;
            let slot = Slot::from(u64::try_from(
                since_genesis.as_millis() / self.slot_duration.as_millis(),
            )?);
            Ok(slot + self.genesis_slot)
        } else {
            Err(anyhow::anyhow!("slot_of: now is less than genesis"))
        }
    }

    /// Returns the duration from `now` until the start of `slot`.
    ///
    /// Will return `None` if `now` is later than the start of `slot`.
    pub fn duration_to_slot(&self, slot: Slot, now: Duration) -> Result<Duration, Error> {
        self.start_of(slot)?
            .checked_sub(now)
            .ok_or(anyhow::anyhow!("duration_to_slot: Subtraction overflow"))
    }

    /// Returns the duration between UNIX epoch and the start of `slot`.
    pub fn start_of(&self, slot: Slot) -> Result<Duration, Error> {
        let slot = slot
            .checked_sub(self.genesis_slot)
            .ok_or(anyhow::anyhow!("start_of: Slot is less than genesis slot"))?
            .try_into()?;
        let unadjusted_slot_duration = self
            .slot_duration
            .checked_mul(slot)
            .ok_or(anyhow::anyhow!("start_of: Multiplication overflow"))?;

        self.genesis_duration
            .checked_add(unadjusted_slot_duration)
            .ok_or(anyhow::anyhow!("start_of: Addition overflow"))
    }

    /// Calculates the current epoch from the genesis time and current time.
    pub fn get_current_epoch(&self) -> Result<Epoch, Error> {
        let now = self.clock.now().duration_since(UNIX_EPOCH)?;
        let slot = self.slot_of(now)?;
        Ok(self.get_epoch_for_slot(slot))
    }

    fn get_epoch_for_slot(&self, slot: Slot) -> Epoch {
        slot / self.slots_per_epoch
    }

    pub fn get_next_epoch_start_slot(&self) -> Result<Slot, Error> {
        let now = self.clock.now().duration_since(UNIX_EPOCH)?;
        let slot = self.slot_of(now)?;
        Ok((self.get_epoch_for_slot(slot) + 1) * self.slots_per_epoch)
    }

    pub fn get_epoch_begin_timestamp(&self, epoch: Epoch) -> Result<u64, Error> {
        let slot = epoch * self.slots_per_epoch;
        let start_of_slot = self.start_of(slot)?;
        Ok(start_of_slot.as_secs())
    }

    pub fn get_current_slot_begin_timestamp(&self) -> Result<u64, Error> {
        let start_of_slot = self.start_of(self.get_current_slot()?)?;
        Ok(start_of_slot.as_secs())
    }

    pub fn get_epoch_for_timestamp(&self, timestamp: u64) -> Result<Epoch, Error> {
        let slot = self.slot_of(Duration::from_secs(timestamp))?;
        Ok(slot / self.slots_per_epoch)
    }

    pub fn get_epoch_from_slot(&self, slot: Slot) -> Epoch {
        slot / self.slots_per_epoch
    }

    pub fn get_current_slot_of_epoch(&self) -> Result<Slot, Error> {
        let now = self.clock.now().duration_since(UNIX_EPOCH)?;
        let cur_slot = self.slot_of(now)?;
        Ok(self.slot_of_epoch(cur_slot))
    }

    pub fn slot_of_epoch(&self, slot: Slot) -> Slot {
        slot % self.slots_per_epoch
    }

    pub fn is_slot_in_last_n_slots_of_epoch(&self, slot: Slot, n: Slot) -> bool {
        slot >= self.slots_per_epoch - n && slot < self.slots_per_epoch
    }

    pub fn time_from_n_last_slots_of_epoch(
        &self,
        l1_slot_of_epoch: Slot,
        n: Slot,
    ) -> Result<Duration, Error> {
        let boundary_slot = self.slots_per_epoch - n;
        if l1_slot_of_epoch < boundary_slot {
            return Err(anyhow::anyhow!(
                "time_from_n_last_slots_of_epoch: too early, slot {} is less than boundary slot {}",
                l1_slot_of_epoch,
                boundary_slot
            ));
        }

        let epoch = self.get_current_epoch()?;
        let boundary_slot_begin = self.start_of(boundary_slot + epoch * self.slots_per_epoch)?;

        Ok(self.clock.now().duration_since(UNIX_EPOCH)? - boundary_slot_begin)
    }

    // 0 based L2 slot number within the current L1 slot
    pub fn get_current_l2_slot_within_l1_slot(&self) -> Result<u64, Error> {
        let l1_slot = self.get_current_slot()?;
        let now = self.clock.now().duration_since(UNIX_EPOCH)?;
        let slot_begin = self.start_of(l1_slot)?;
        Ok(self.which_l2_slot_is_it(u64::try_from((now - slot_begin).as_millis())?))
    }

    pub fn get_l2_slot_begin_timestamp(&self) -> Result<u64, Error> {
        let now = self.clock.now().duration_since(UNIX_EPOCH)?;
        let now_from_genesis = now - self.genesis_duration;
        let preconf_heartbeat_ms: u128 = u128::from(self.preconf_heartbeat_ms);
        let timestamp_sec = TryInto::<u64>::try_into(
            ((now_from_genesis.as_millis() / preconf_heartbeat_ms) * preconf_heartbeat_ms) / 1000,
        )
        .map_err(|_| anyhow::anyhow!("get_l2_slot_begin_timestamp: Conversion overflow"))?
            + self.genesis_duration.as_secs();
        Ok(timestamp_sec)
    }

    fn which_l2_slot_is_it(&self, ms_from_l1_slot_begin: u64) -> u64 {
        ms_from_l1_slot_begin / self.preconf_heartbeat_ms
    }

    pub fn get_l2_slots_per_epoch(&self) -> u64 {
        self.slots_per_epoch * self.l2_slots_per_l1
    }

    pub fn slots_since_l1_block(&self, l1_block_timestamp: u64) -> Result<u64, Error> {
        let block_slot = self.slot_of(Duration::from_secs(l1_block_timestamp))?;
        let current_slot = self.get_current_slot()?;

        tracing::trace!(
            "slots_since_l1_block: block slot {}, current slot {}",
            block_slot,
            current_slot
        );

        if current_slot < block_slot {
            return Err(anyhow::anyhow!(
                "slots_since_l1_block: Current slot is less than block slot"
            ));
        }

        Ok(current_slot - block_slot)
    }
}

#[cfg(test)]
pub mod mock {
    use super::*;
    use chrono::DateTime;

    #[derive(Default)]
    pub struct MockClock {
        pub timestamp: i64,
    }
    impl Clock for MockClock {
        fn now(&self) -> SystemTime {
            SystemTime::from(DateTime::from_timestamp(self.timestamp, 0).unwrap())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::*;
    use super::*;
    use chrono::DateTime;
    use k256::pkcs8::der::Decode;

    const SLOTS_PER_EPOCH: u64 = 32;
    const SLOT_DURATION: u64 = 12;
    const PRECONF_HEART_BEAT_MS: u64 = 3000;

    #[test]
    fn test_duration_to_next_slot() {
        let slot_clock: SlotClock = SlotClock::new(0, 5, SLOT_DURATION, 32, PRECONF_HEART_BEAT_MS);

        let now = Duration::from_secs(10);
        let duration_to_next_slot = slot_clock.duration_to_next_slot_from(now).unwrap();
        assert_eq!(duration_to_next_slot, Duration::from_secs(7));
    }

    #[test]
    fn test_slot_of() {
        let slot_clock: SlotClock =
            SlotClock::new(0u64, 0, SLOT_DURATION, 32, PRECONF_HEART_BEAT_MS);

        let now = Duration::from_secs(25);
        let slot = slot_clock.slot_of(now).unwrap();
        assert_eq!(slot, Slot::from(2u64));
    }

    #[test]
    fn test_duration_to_slot() {
        let slot_clock: SlotClock =
            SlotClock::new(0u64, 0, SLOT_DURATION, 32, PRECONF_HEART_BEAT_MS);

        let now = Duration::from_secs(10);
        let slot = Slot::from(2u64);
        let duration_to_slot = slot_clock.duration_to_slot(slot, now).unwrap();
        assert_eq!(duration_to_slot, Duration::from_secs(14));
    }

    #[test]
    fn test_start_of() {
        let slot_clock: SlotClock =
            SlotClock::new(0u64, 0, SLOT_DURATION, 32, PRECONF_HEART_BEAT_MS);

        let start_of_slot = slot_clock.start_of(Slot::from(3u64)).unwrap();
        assert_eq!(start_of_slot, Duration::from_secs(36));
    }

    #[test]
    fn test_get_current_slot() {
        let genesis_slot = Slot::from(0u64);
        let slot_clock: SlotClock = SlotClock::new(
            genesis_slot,
            1721387493,
            SLOT_DURATION,
            32,
            PRECONF_HEART_BEAT_MS,
        );

        let current_slot = slot_clock.get_current_slot().unwrap();
        println!("current_slot: {current_slot}");
        assert!(current_slot > genesis_slot);
    }

    #[test]
    fn test_get_epoch_for_slot() {
        let slot_clock: SlotClock =
            SlotClock::new(0u64, 0, SLOT_DURATION, 32, PRECONF_HEART_BEAT_MS);
        let epoch = slot_clock.get_epoch_for_slot(Slot::from(3u64));
        assert_eq!(epoch, Epoch::from(0u64));

        let epoch = slot_clock.get_epoch_for_slot(Slot::from(234u64));
        assert_eq!(epoch, Epoch::from(7u64));
    }

    #[test]
    fn test_get_current_epoch() {
        let genesis_slot = Slot::from(0u64);
        let slot_clock: SlotClock = SlotClock::new(
            genesis_slot,
            1721387493,
            SLOT_DURATION,
            32,
            PRECONF_HEART_BEAT_MS,
        );

        let current_epoch = slot_clock.get_current_epoch().unwrap();
        assert!(current_epoch > 0);
    }

    #[test]
    fn test_get_epoch_begin_timestamp() {
        let genesis_slot = Slot::from(0u64);
        let genesis_timestamp = 100;
        let slot_duration = SLOT_DURATION;
        let slot_per_epoch = SLOTS_PER_EPOCH;
        let slot_clock: SlotClock = SlotClock::new(
            genesis_slot,
            genesis_timestamp,
            slot_duration,
            slot_per_epoch,
            PRECONF_HEART_BEAT_MS,
        );

        let epoch_begin_timestamp = slot_clock.get_epoch_begin_timestamp(1).unwrap();
        assert_eq!(
            epoch_begin_timestamp,
            genesis_timestamp + slot_per_epoch * slot_duration
        );
    }

    #[test]
    fn test_get_current_slot_of_epoch() {
        let slot_clock: SlotClock =
            SlotClock::new(0u64, 0, SLOT_DURATION, 32, PRECONF_HEART_BEAT_MS);

        assert_eq!(slot_clock.slot_of_epoch(1234), 18);
        assert_eq!(slot_clock.slot_of_epoch(293482), 10);
    }

    #[test]
    fn test_is_current_slot_in_last_n_slots_of_epoch() {
        let slot_clock: SlotClock =
            SlotClock::new(0u64, 0, SLOT_DURATION, 32, PRECONF_HEART_BEAT_MS);

        assert!(!slot_clock.is_slot_in_last_n_slots_of_epoch(0, 2));
        assert!(!slot_clock.is_slot_in_last_n_slots_of_epoch(1, 2));
        assert!(!slot_clock.is_slot_in_last_n_slots_of_epoch(29, 2));
        assert!(slot_clock.is_slot_in_last_n_slots_of_epoch(30, 2));
        assert!(slot_clock.is_slot_in_last_n_slots_of_epoch(31, 2));
        assert!(!slot_clock.is_slot_in_last_n_slots_of_epoch(32, 2));
    }

    #[test]
    fn test_time_from_n_last_slots_of_epoch() {
        #[derive(Default)]
        pub struct MockClock;
        impl Clock for MockClock {
            fn now(&self) -> SystemTime {
                SystemTime::from(DateTime::from_timestamp(353, 0).unwrap())
            }
        }

        let slot_clock =
            SlotClock::<MockClock>::new(0u64, 0, SLOT_DURATION, 32, PRECONF_HEART_BEAT_MS);

        let duration = slot_clock.time_from_n_last_slots_of_epoch(29, 3).unwrap();
        assert_eq!(duration, Duration::from_secs(5));

        #[derive(Default)]
        pub struct MockClock2;
        impl Clock for MockClock2 {
            fn now(&self) -> SystemTime {
                SystemTime::from(DateTime::from_timestamp(737, 0).unwrap())
            }
        }

        let slot_clock =
            SlotClock::<MockClock2>::new(0u64, 0, SLOT_DURATION, 32, PRECONF_HEART_BEAT_MS);

        let duration = slot_clock.time_from_n_last_slots_of_epoch(29, 3).unwrap();
        assert_eq!(duration, Duration::from_secs(5));
    }

    #[test]
    fn test_get_l2_slot_number_within_l1_slot() {
        let mut slot_clock: SlotClock<MockClock> =
            SlotClock::<MockClock>::new(0u64, 0, SLOT_DURATION, 32, PRECONF_HEART_BEAT_MS);
        slot_clock.clock.timestamp = 36;

        let l2_slot_number_within_l1_slot =
            slot_clock.get_current_l2_slot_within_l1_slot().unwrap();
        assert_eq!(l2_slot_number_within_l1_slot, 0);

        slot_clock.clock.timestamp = 44;
        let l2_slot_number_within_l1_slot =
            slot_clock.get_current_l2_slot_within_l1_slot().unwrap();
        assert_eq!(l2_slot_number_within_l1_slot, 2);
    }

    #[test]
    fn test_get_l2_slot_begin_timestamp() {
        let mut slot_clock =
            SlotClock::<MockClock>::new(0u64, 5, SLOT_DURATION, 32, PRECONF_HEART_BEAT_MS);

        slot_clock.clock.timestamp = 22;
        assert_eq!(slot_clock.get_l2_slot_begin_timestamp().unwrap(), 20);

        slot_clock.clock.timestamp = 23;
        assert_eq!(slot_clock.get_l2_slot_begin_timestamp().unwrap(), 23);

        slot_clock.clock.timestamp = 24;
        assert_eq!(slot_clock.get_l2_slot_begin_timestamp().unwrap(), 23);

        slot_clock.clock.timestamp = 25;
        assert_eq!(slot_clock.get_l2_slot_begin_timestamp().unwrap(), 23);

        slot_clock.clock.timestamp = 26;
        assert_eq!(slot_clock.get_l2_slot_begin_timestamp().unwrap(), 26);
    }

    #[test]
    fn test_get_l2_slots_per_epoch() {
        let slot_clock: SlotClock = SlotClock::new(0, 0, SLOT_DURATION, 32, PRECONF_HEART_BEAT_MS);
        assert_eq!(slot_clock.get_l2_slots_per_epoch(), 32 * 4);
    }

    #[test]
    fn test_slots_since_l1_block() {
        #[derive(Default)]
        pub struct MockClock;
        impl Clock for MockClock {
            fn now(&self) -> SystemTime {
                SystemTime::from(DateTime::from_timestamp(96, 0).unwrap()) // 8th slot
            }
        }

        let slot_clock =
            SlotClock::<MockClock>::new(0u64, 0, SLOT_DURATION, 32, PRECONF_HEART_BEAT_MS);

        // Test case 1: L1 block timestamp is at slot 6
        assert_eq!(slot_clock.slots_since_l1_block(72).unwrap(), 2);

        // Test case 2: L1 block timestamp is at slot 3
        assert_eq!(slot_clock.slots_since_l1_block(36).unwrap(), 5);

        // Test case 3: L1 block timestamp is at slot 8
        assert_eq!(slot_clock.slots_since_l1_block(96).unwrap(), 0);
    }

    #[test]
    fn test_get_epoch_duration() {
        let slot_clock: SlotClock = SlotClock::new(0, 0, SLOT_DURATION, 32, PRECONF_HEART_BEAT_MS);
        assert_eq!(slot_clock.get_epoch_duration(), Duration::from_secs(384));
    }
}
