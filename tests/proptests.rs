#![cfg(not(feature = "loom"))]

use std::collections::BTreeMap;

use cachelog::{CacheLogConfig, CacheLogMap, DirtyWriteMode, EntryState};
use proptest::prelude::*;

fn map_for_mode(mode: DirtyWriteMode) -> CacheLogMap<usize, usize> {
    CacheLogMap::new(CacheLogConfig::new(128, 128, 64).with_dirty_write_mode(mode))
}

fn drain_all(map: &CacheLogMap<usize, usize>) {
    loop {
        let batch = map.low_level().flush_batch(64);
        if batch.is_empty() {
            break;
        }
        let marked = map.low_level().mark_flushed(&batch);
        assert!(marked > 0);
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, .. ProptestConfig::default() })]

    #[test]
    fn prop_dirty_writes_are_last_write_wins_then_flush_clears(
        mode_pick in 0u8..=1,
        ops in prop::collection::vec((0u8..=15, 0u16..=2047), 1..128)
    ) {
        let mode = if mode_pick == 0 {
            DirtyWriteMode::StrictLog
        } else {
            DirtyWriteMode::CoalescedMap
        };

        let map = map_for_mode(mode);
        let mut expected = BTreeMap::<usize, usize>::new();

        for (k, v) in ops {
            let key = usize::from(k);
            let val = usize::from(v);
            let _ = map.low_level().insert_dirty(key, val);
            expected.insert(key, val);
        }

        for (k, v) in &expected {
            let got = map.read(k, |_, value, state| (*value, state));
            prop_assert_eq!(got, Some((*v, EntryState::Dirty)));
        }

        drain_all(&map);

        for k in expected.keys() {
            prop_assert!(map.read(k, |_, value, state| (*value, state)).is_none());
        }
        prop_assert_eq!(map.dirty_log_len(), 0);
    }

    #[test]
    fn prop_insert_dirty_batch_without_ids_count_matches_mode(
        mode_pick in 0u8..=1,
        batch in prop::collection::vec((0u8..=31, 0u16..=4095), 1..80)
    ) {
        let mode = if mode_pick == 0 {
            DirtyWriteMode::StrictLog
        } else {
            DirtyWriteMode::CoalescedMap
        };

        let map = map_for_mode(mode);
        let mut expected_last = BTreeMap::<usize, usize>::new();
        let mut expected_unique = BTreeMap::<usize, ()>::new();
        let mut owned = Vec::with_capacity(batch.len());

        for (k, v) in &batch {
            let key = usize::from(*k);
            let val = usize::from(*v);
            expected_last.insert(key, val);
            expected_unique.insert(key, ());
            owned.push((key, val));
        }

        let inserted = map.low_level().insert_dirty_batch_without_ids(owned);
        match mode {
            DirtyWriteMode::StrictLog => prop_assert_eq!(inserted, batch.len()),
            DirtyWriteMode::CoalescedMap => prop_assert_eq!(inserted, expected_unique.len()),
        }

        for (k, v) in &expected_last {
            let got = map.read(k, |_, value, state| (*value, state));
            prop_assert_eq!(got, Some((*v, EntryState::Dirty)));
        }

        drain_all(&map);
        prop_assert_eq!(map.dirty_log_len(), 0);
    }
}
