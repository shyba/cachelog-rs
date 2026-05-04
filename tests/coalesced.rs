#![cfg(not(feature = "loom"))]

mod common;

use bytes::Bytes;
use cachelog::{CacheLogConfig, CacheLogMap, DirtyWriteMode, EntryState};
use common::read_pair;

#[test]
fn coalesced_dirty_write_counts_as_visible_for_helpers() {
    let map = CacheLogMap::<String, usize>::new(
        CacheLogConfig::new(16, 16, 16).with_dirty_write_mode(DirtyWriteMode::CoalescedMap),
    );

    map.put("alpha".to_owned(), 1);

    assert!(map.contains("alpha"));
    assert_eq!(map.visible_len(), 1);
    assert!(map.cleanup_stale_visible("alpha"));
}

#[test]
fn coalesced_visible_len_dedupes_rewritten_inflight_keys() {
    let map = CacheLogMap::<String, usize>::new(
        CacheLogConfig::new(16, 16, 16).with_dirty_write_mode(DirtyWriteMode::CoalescedMap),
    );

    map.put("alpha".to_owned(), 1);
    let batch = map.low_level().flush_batch(16);
    assert_eq!(batch.len(), 1);
    map.put("alpha".to_owned(), 2);

    assert_eq!(map.visible_len(), 1);
    assert_eq!(map.get_cloned("alpha"), Some((2, EntryState::Dirty)));
}

#[test]
fn coalesced_dirty_backlog_counts_track_active_inflight_and_draining() {
    let map = CacheLogMap::<String, usize>::new(
        CacheLogConfig::new(16, 16, 16).with_dirty_write_mode(DirtyWriteMode::CoalescedMap),
    );

    map.put("alpha".to_owned(), 1);
    map.put("beta".to_owned(), 2);
    assert_eq!(
        map.dirty_backlog_counts(),
        cachelog::DirtyBacklogCounts {
            dirty_total: 2,
            pending_visible: 2,
            inflight: 0,
            draining: 0,
        }
    );

    let batch = map.low_level().flush_batch(1);
    assert_eq!(batch.len(), 1);
    assert_eq!(
        map.dirty_backlog_counts(),
        cachelog::DirtyBacklogCounts {
            dirty_total: 2,
            pending_visible: 0,
            inflight: 1,
            draining: 1,
        }
    );

    assert_eq!(map.low_level().mark_flushed(&batch), 1);
    assert_eq!(
        map.dirty_backlog_counts(),
        cachelog::DirtyBacklogCounts {
            dirty_total: 1,
            pending_visible: 0,
            inflight: 0,
            draining: 1,
        }
    );
}

#[test]
fn coalesced_dirty_log_len_tracks_visible_dirty_backlog() {
    let cfg = CacheLogConfig::new(32, 32, 32).with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
    let map = CacheLogMap::<String, usize>::new(cfg);

    map.low_level().insert_dirty("a".to_owned(), 1);
    map.low_level().insert_dirty("b".to_owned(), 2);
    map.low_level().insert_dirty("a".to_owned(), 3);

    assert_eq!(map.dirty_log_len(), 2);

    let batch = map.low_level().flush_batch(16);
    assert_eq!(map.low_level().mark_flushed(&batch), batch.len());
    assert_eq!(map.dirty_log_len(), 0);
    assert!(
        map.low_level()
            .read_full(&"a".to_owned(), |_, v, s, r| (*v, s, r))
            .is_none()
    );
    assert!(
        map.low_level()
            .read_full(&"b".to_owned(), |_, v, s, r| (*v, s, r))
            .is_none()
    );
}

#[test]
fn insert_dirty_batch_coalesced_keeps_last_per_key_for_batch() {
    let cfg = CacheLogConfig::new(32, 32, 32).with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
    let map = CacheLogMap::<String, usize>::new(cfg);

    let ids = map.low_level().insert_dirty_batch(vec![
        ("same".to_owned(), 1),
        ("same".to_owned(), 2),
        ("other".to_owned(), 9),
        ("same".to_owned(), 3),
    ]);

    assert_eq!(ids.len(), 2);
    assert!(matches!(
        read_pair(&map, &"same".to_owned()),
        Some((3, EntryState::Dirty))
    ));
    assert!(matches!(
        read_pair(&map, &"other".to_owned()),
        Some((9, EntryState::Dirty))
    ));

    let batch = map.low_level().flush_batch(10);
    assert_eq!(batch.len(), 2);
    let mut rows = batch
        .iter()
        .map(|entry| (entry.key.clone(), entry.value))
        .collect::<Vec<_>>();
    rows.sort();
    assert_eq!(rows, vec![("other".to_owned(), 9), ("same".to_owned(), 3)]);
}

#[test]
fn bytes_batch_coalesces_duplicate_keys() {
    let config =
        CacheLogConfig::new(64, 64, 64).with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
    let map = CacheLogMap::<Vec<u8>, Bytes>::new(config);

    let written = map.low_level().insert_dirty_batch_without_ids(vec![
        (b"dup".to_vec(), Bytes::from_static(b"\x01")),
        (b"keep".to_vec(), Bytes::from_static(b"\x02")),
        (b"dup".to_vec(), Bytes::from_static(b"\x03")),
    ]);

    assert_eq!(written, 2);
    assert_eq!(map.dirty_log_len(), 2);
    assert_eq!(
        map.read(b"dup".as_slice(), |_, value, _| value.clone()),
        Some(Bytes::from_static(b"\x03"))
    );
    assert_eq!(
        map.read(b"keep".as_slice(), |_, value, _| value.clone()),
        Some(Bytes::from_static(b"\x02"))
    );
}
