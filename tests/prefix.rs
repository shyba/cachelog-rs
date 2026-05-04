#![cfg(not(feature = "loom"))]

mod common;

use std::collections::BTreeSet;

use cachelog::{CacheLogConfig, CacheLogMap};
use common::PrefixOrderBuildHasher;

#[test]
fn list_prefix_scans_visible_entries_without_flush() {
    let map = CacheLogMap::<Vec<u8>, usize>::new(CacheLogConfig::new(64, 64, 64));
    map.low_level().insert_dirty(b"ab:1".to_vec(), 1);
    map.low_level().insert_dirty(b"ab:2".to_vec(), 2);
    map.low_level().insert_dirty(b"ac:1".to_vec(), 3);
    assert_eq!(map.insert_clean_if_absent(b"ab:clean".to_vec(), 4), Some(0));

    let rows = map.list_prefix(b"ab:", |key, value, state| (key.clone(), *value, state), 16);
    assert_eq!(rows.len(), 3);

    let keys = rows.into_iter().map(|(k, _, _)| k).collect::<BTreeSet<_>>();
    assert!(keys.contains(b"ab:1".as_slice()));
    assert!(keys.contains(b"ab:2".as_slice()));
    assert!(keys.contains(b"ab:clean".as_slice()));
}

#[test]
fn list_prefix_returns_keys_in_lexicographic_order() {
    let map = CacheLogMap::<Vec<u8>, usize>::new(CacheLogConfig::new(64, 64, 64));
    map.low_level().insert_dirty(b"ab:20".to_vec(), 20);
    map.low_level().insert_dirty(b"ab:03".to_vec(), 3);
    map.low_level().insert_dirty(b"ab:11".to_vec(), 11);

    let keys = map.list_prefix(b"ab:", |key, _, _| key.clone(), 16);
    assert_eq!(
        keys,
        vec![b"ab:03".to_vec(), b"ab:11".to_vec(), b"ab:20".to_vec()]
    );
}

#[test]
fn with_prefix_snapshot_returns_sorted_matching_keys() {
    let map = CacheLogMap::<Vec<u8>, usize>::new(CacheLogConfig::new(64, 64, 64));
    map.low_level().insert_dirty(b"ab:20".to_vec(), 20);
    map.low_level().insert_dirty(b"ab:03".to_vec(), 3);
    map.low_level().insert_dirty(b"zz:01".to_vec(), 1);
    map.low_level().insert_dirty(b"ab:11".to_vec(), 11);

    let keys = map.with_prefix_snapshot(b"ab:", 16, |keys| keys.to_vec());
    assert_eq!(
        keys,
        vec![b"ab:03".to_vec(), b"ab:11".to_vec(), b"ab:20".to_vec()]
    );
}

#[test]
fn with_prefix_snapshot_returns_lexicographically_first_limit_keys() {
    let map = CacheLogMap::<Vec<u8>, usize, PrefixOrderBuildHasher>::with_hasher(
        CacheLogConfig::new(64, 64, 64),
        PrefixOrderBuildHasher,
    );
    map.low_level().insert_dirty(b"ab:a".to_vec(), 1);
    map.low_level().insert_dirty(b"ab:b".to_vec(), 2);
    map.low_level().insert_dirty(b"ab:c".to_vec(), 3);

    let keys = map.with_prefix_snapshot(b"ab:", 2, |keys| keys.to_vec());
    assert_eq!(keys, vec![b"ab:a".to_vec(), b"ab:b".to_vec()]);
}

#[test]
fn list_prefix_respects_limit() {
    let map = CacheLogMap::<Vec<u8>, usize>::new(CacheLogConfig::new(64, 64, 64));
    map.low_level().insert_dirty(b"ab:1".to_vec(), 1);
    map.low_level().insert_dirty(b"ab:2".to_vec(), 2);
    map.low_level().insert_dirty(b"ab:3".to_vec(), 3);

    let rows = map.list_prefix(b"ab:", |_, value, _| *value, 2);
    assert_eq!(rows.len(), 2);
}

#[test]
fn for_each_prefix_reports_match_count_without_allocating_results() {
    let map = CacheLogMap::<Vec<u8>, usize>::new(CacheLogConfig::new(64, 64, 64));
    map.low_level().insert_dirty(b"ab:1".to_vec(), 1);
    map.low_level().insert_dirty(b"ab:2".to_vec(), 2);
    map.low_level().insert_dirty(b"zz:1".to_vec(), 3);

    let mut sum = 0_usize;
    let matched = map.for_each_prefix(
        b"ab:",
        |_, value, state| {
            assert_eq!(state, cachelog::EntryState::Dirty);
            sum += *value;
        },
        16,
    );

    assert_eq!(matched, 2);
    assert_eq!(sum, 3);
}

#[test]
fn for_each_prefix_respects_limit() {
    let map = CacheLogMap::<Vec<u8>, usize>::new(CacheLogConfig::new(64, 64, 64));
    map.low_level().insert_dirty(b"ab:1".to_vec(), 1);
    map.low_level().insert_dirty(b"ab:2".to_vec(), 2);
    map.low_level().insert_dirty(b"ab:3".to_vec(), 3);

    let mut seen = 0_usize;
    let matched = map.for_each_prefix(
        b"ab:",
        |_, _, _| {
            seen += 1;
        },
        2,
    );
    assert_eq!(matched, 2);
    assert_eq!(seen, 2);
}

#[test]
fn for_each_prefix_key_returns_only_matching_keys() {
    let map = CacheLogMap::<Vec<u8>, usize>::new(CacheLogConfig::new(64, 64, 64));
    map.low_level().insert_dirty(b"ab:1".to_vec(), 1);
    map.low_level().insert_dirty(b"ab:2".to_vec(), 2);
    map.low_level().insert_dirty(b"zz:1".to_vec(), 3);

    let mut keys = BTreeSet::new();
    let matched = map.for_each_prefix_key(
        b"ab:",
        |k| {
            keys.insert(k.clone());
        },
        16,
    );
    assert_eq!(matched, 2);
    assert!(keys.contains(b"ab:1".as_slice()));
    assert!(keys.contains(b"ab:2".as_slice()));
    assert!(!keys.contains(b"zz:1".as_slice()));
}
