#![cfg(not(feature = "loom"))]

use std::borrow::Borrow;
use std::collections::BTreeSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{BuildHasher, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;

use bytes::Bytes;
use cachelog::{
    BytePrefixMap, CacheLogConfig, CacheLogMap, DirtyWriteMode, EntryState, FlushWork,
    low_level::VisibleRef,
};

fn read_pair<K, Q, V>(map: &CacheLogMap<K, V>, key: &Q) -> Option<(V, EntryState)>
where
    K: Borrow<Q> + Clone + Eq + std::hash::Hash,
    Q: Eq + std::hash::Hash + ?Sized,
    V: Copy,
{
    map.read(key, |_, value, state| (*value, state))
}

fn read_triplet<K, Q, V>(map: &CacheLogMap<K, V>, key: &Q) -> Option<(V, EntryState, VisibleRef)>
where
    K: Borrow<Q> + Clone + Eq + std::hash::Hash,
    Q: Eq + std::hash::Hash + ?Sized,
    V: Copy,
{
    map.low_level()
        .read_full(key, |_, value, state, visible| (*value, state, visible))
}

#[derive(Clone, Default)]
struct PrefixOrderBuildHasher;

#[derive(Default)]
struct PrefixOrderHasher {
    bytes: Vec<u8>,
}

impl Hasher for PrefixOrderHasher {
    fn finish(&self) -> u64 {
        match self.bytes.iter().rev().find(|&&b| b.is_ascii_lowercase()) {
            Some(b'b') => 0,
            Some(b'c') => 1,
            Some(b'a') => 2,
            Some(other) => u64::from(*other),
            None => DefaultHasher::new().finish(),
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }
}

impl BuildHasher for PrefixOrderBuildHasher {
    type Hasher = PrefixOrderHasher;

    fn build_hasher(&self) -> Self::Hasher {
        PrefixOrderHasher::default()
    }
}

// Common product coverage: point reads, prefix/list behavior, and common
// persist helpers that should hold regardless of dirty-write mode internals.

#[test]
fn dirty_write_is_immediately_visible() {
    let map = CacheLogMap::<String, usize>::new(CacheLogConfig::new(16, 16, 16));

    map.put("alpha".to_owned(), 1);
    assert_eq!(
        read_pair(&map, &"alpha".to_owned()),
        Some((1, EntryState::Dirty))
    );
}

#[test]
fn strict_dirty_writes_do_not_block_when_queue_capacity_is_full() {
    let map = CacheLogMap::<String, usize>::new(CacheLogConfig::new(16, 1, 16));

    map.put("alpha".to_owned(), 1);
    map.put("beta".to_owned(), 2);

    assert_eq!(map.dirty_log_len(), 2);
    assert_eq!(
        read_pair(&map, &"alpha".to_owned()),
        Some((1, EntryState::Dirty))
    );
    assert_eq!(
        read_pair(&map, &"beta".to_owned()),
        Some((2, EntryState::Dirty))
    );
}

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
fn borrowed_string_lookup_works_across_common_api() {
    let map = CacheLogMap::<String, usize>::new(CacheLogConfig::new(16, 16, 16));
    map.put("borrowed".to_owned(), 9);

    assert!(map.contains("borrowed"));
    assert_eq!(map.get_cloned("borrowed"), Some((9, EntryState::Dirty)));
    assert_eq!(
        map.read("borrowed", |key, value, state| {
            (key.clone(), *value, state)
        }),
        Some(("borrowed".to_owned(), 9, EntryState::Dirty))
    );
    assert!(map.cleanup_stale_visible("borrowed"));
    assert!(!map.contains("borrowed"));
}

#[test]
fn borrowed_bytes_lookup_works_across_common_api() {
    let map = CacheLogMap::<Vec<u8>, usize>::new(CacheLogConfig::new(16, 16, 16));
    map.put(b"bytes".to_vec(), 11);

    assert!(map.contains(b"bytes".as_slice()));
    assert_eq!(
        map.get_cloned(b"bytes".as_slice()),
        Some((11, EntryState::Dirty))
    );
    assert_eq!(
        map.read(b"bytes".as_slice(), |key, value, state| {
            (key.clone(), *value, state)
        }),
        Some((b"bytes".to_vec(), 11, EntryState::Dirty))
    );
    assert!(map.cleanup_stale_visible(b"bytes".as_slice()));
    assert!(!map.contains(b"bytes".as_slice()));
}

#[test]
fn bytes_value_common_reads_report_state_without_visible_refs() {
    let map = CacheLogMap::<Vec<u8>, Bytes>::new(CacheLogConfig::new(16, 16, 16));

    map.put(b"bytes:1".to_vec(), Bytes::from_static(b"value:1"));
    map.put(b"bytes:2".to_vec(), Bytes::from_static(b"value:2"));

    assert_eq!(
        map.read(b"bytes:1".as_slice(), |_, value, state| {
            (value.clone(), state)
        }),
        Some((Bytes::from_static(b"value:1"), EntryState::Dirty))
    );
    assert_eq!(
        map.read(b"bytes:2".as_slice(), |_, value, state| {
            (value.clone(), state)
        }),
        Some((Bytes::from_static(b"value:2"), EntryState::Dirty))
    );
}

#[test]
fn clean_insert_requires_absence() {
    let map = CacheLogMap::<String, usize>::new(CacheLogConfig::new(16, 16, 16));

    assert_eq!(map.insert_clean_if_absent("beta".to_owned(), 7), Some(0));
    assert_eq!(map.insert_clean_if_absent("beta".to_owned(), 8), None);
    assert_eq!(
        read_pair(&map, &"beta".to_owned()),
        Some((7, EntryState::Clean))
    );
}

#[test]
fn dirty_write_replaces_visible_clean_entry() {
    let map = CacheLogMap::<String, usize>::new(CacheLogConfig::new(16, 16, 16));

    assert_eq!(map.insert_clean_if_absent("gamma".to_owned(), 3), Some(0));
    map.put("gamma".to_owned(), 4);

    assert_eq!(
        read_pair(&map, &"gamma".to_owned()),
        Some((4, EntryState::Dirty))
    );
}

#[test]
fn coalesced_clean_insert_rejects_pending_dirty_entry() {
    let map = CacheLogMap::<String, usize>::new(
        CacheLogConfig::new(16, 16, 16).with_dirty_write_mode(DirtyWriteMode::CoalescedMap),
    );

    map.put("gamma".to_owned(), 4);
    assert_eq!(map.insert_clean_if_absent("gamma".to_owned(), 3), None);
    assert_eq!(
        read_pair(&map, &"gamma".to_owned()),
        Some((4, EntryState::Dirty))
    );

    let batch = map.low_level().flush_batch(16);
    assert_eq!(map.low_level().mark_flushed(&batch), 1);
    assert_eq!(read_pair(&map, &"gamma".to_owned()), None);
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

// Strict low-level coverage: ordered ids, visible refs, and flush/mark
// mechanics that intentionally exercise the id-bearing surface.

#[test]
fn borrowed_string_lookup_works_across_low_level_api() {
    let map = CacheLogMap::<String, usize>::new(CacheLogConfig::new(16, 16, 16));
    let id = map.low_level().insert_dirty("borrowed".to_owned(), 9);

    assert_eq!(
        map.low_level().visible_ref("borrowed"),
        Some(VisibleRef::Dirty(id))
    );
    assert_eq!(
        map.low_level().get_cloned_full("borrowed"),
        Some((9, EntryState::Dirty, VisibleRef::Dirty(id)))
    );
    assert_eq!(
        map.low_level()
            .read_full("borrowed", |key, value, state, visible| {
                (key.clone(), *value, state, visible)
            }),
        Some((
            "borrowed".to_owned(),
            9,
            EntryState::Dirty,
            VisibleRef::Dirty(id)
        ))
    );
}

#[test]
fn borrowed_bytes_lookup_works_across_low_level_api() {
    let map = CacheLogMap::<Vec<u8>, usize>::new(CacheLogConfig::new(16, 16, 16));
    let id = map.low_level().insert_dirty(b"bytes".to_vec(), 11);

    assert_eq!(
        map.low_level().visible_ref(b"bytes".as_slice()),
        Some(VisibleRef::Dirty(id))
    );
    assert_eq!(
        map.low_level().get_cloned_full(b"bytes".as_slice()),
        Some((11, EntryState::Dirty, VisibleRef::Dirty(id)))
    );
    assert_eq!(
        map.low_level()
            .read_full(b"bytes".as_slice(), |key, value, state, visible| {
                (key.clone(), *value, state, visible)
            }),
        Some((
            b"bytes".to_vec(),
            11,
            EntryState::Dirty,
            VisibleRef::Dirty(id)
        ))
    );
}

#[test]
fn bytes_value_product_paths_work() {
    let map = CacheLogMap::<Vec<u8>, Bytes>::new(CacheLogConfig::new(16, 16, 16));

    let id1 = map
        .low_level()
        .insert_dirty(b"bytes:1".to_vec(), Bytes::from_static(b"value:1"));
    assert_eq!(
        map.low_level()
            .read_full(b"bytes:1".as_slice(), |_, value, state, visible| {
                (value.clone(), state, visible)
            }),
        Some((
            Bytes::from_static(b"value:1"),
            EntryState::Dirty,
            VisibleRef::Dirty(id1)
        ))
    );

    let id2 = map
        .low_level()
        .insert_dirty(b"bytes:2".to_vec(), Bytes::from_static(b"value:2"));
    assert_eq!(
        map.low_level()
            .read_full(b"bytes:2".as_slice(), |_, value, state, visible| {
                (value.clone(), state, visible)
            }),
        Some((
            Bytes::from_static(b"value:2"),
            EntryState::Dirty,
            VisibleRef::Dirty(id2)
        ))
    );

    let inserted = map.low_level().insert_dirty_batch_without_ids(vec![
        (b"bytes:b1".to_vec(), Bytes::from_static(b"v1")),
        (b"bytes:b2".to_vec(), Bytes::from_static(b"v2")),
    ]);
    assert_eq!(inserted, 2);
}

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
            assert_eq!(state, EntryState::Dirty);
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

#[test]
fn flush_batch_is_in_write_order() {
    let map = CacheLogMap::<String, usize>::new(CacheLogConfig::new(16, 16, 16));

    map.low_level().insert_dirty("a".to_owned(), 1);
    map.low_level().insert_dirty("b".to_owned(), 2);
    map.low_level().insert_dirty("c".to_owned(), 3);

    let batch = map.low_level().flush_batch(2);
    let ids = batch.last_id().into_iter().collect::<Vec<_>>();
    let keys = batch
        .iter()
        .map(|entry| entry.key.clone())
        .collect::<Vec<_>>();
    assert_eq!(ids.len(), 1);
    assert_eq!(keys, vec!["a".to_owned(), "b".to_owned()]);
}

#[test]
fn mark_flushed_drops_flushed_prefix_immediately() {
    let map = CacheLogMap::<String, usize>::new(CacheLogConfig::new(16, 16, 16));

    map.low_level().insert_dirty("same".to_owned(), 1);
    map.low_level().insert_dirty("same".to_owned(), 2);

    let batch = map.low_level().flush_batch(2);
    assert_eq!(map.low_level().mark_flushed(&batch), 2);
    assert_eq!(map.dirty_log_len(), 0);
    drop(batch);
    assert_eq!(map.read(&"same".to_owned(), |_, _, _| ()), None);
    assert_eq!(map.low_level().visible_ref(&"same".to_owned()), None);
}

#[test]
fn with_flush_batch_persists_and_marks_committed() {
    let map = CacheLogMap::<String, usize>::new(CacheLogConfig::new(16, 16, 16));
    map.low_level().insert_dirty("a".to_owned(), 1);
    map.low_level().insert_dirty("b".to_owned(), 2);

    let mut persisted = Vec::new();
    let committed = map
        .with_flush_batch(2, |batch| {
            persisted = batch
                .iter()
                .map(|entry| (entry.key().clone(), *entry.value()))
                .collect();
            Ok::<_, ()>(())
        })
        .unwrap();

    assert_eq!(committed, 2);
    assert_eq!(persisted, vec![("a".to_owned(), 1), ("b".to_owned(), 2)]);
    assert_eq!(map.dirty_log_len(), 0);
}

#[test]
fn with_persisted_scan_flushes_before_scanning_and_returns_scan_result() {
    let map = CacheLogMap::<String, usize>::new(CacheLogConfig::new(16, 16, 16));
    map.low_level().insert_dirty("a".to_owned(), 1);
    map.low_level().insert_dirty("b".to_owned(), 2);

    let mut persisted = Vec::new();
    let result = map
        .with_persisted_scan(
            2,
            |batch| {
                persisted = batch
                    .iter()
                    .map(|entry| (entry.key().clone(), *entry.value()))
                    .collect();
                Ok::<_, &'static str>(())
            },
            || {
                assert_eq!(map.dirty_log_len(), 0);
                Ok::<_, &'static str>("scan-ok")
            },
        )
        .unwrap();

    assert_eq!(result, "scan-ok");
    assert_eq!(persisted, vec![("a".to_owned(), 1), ("b".to_owned(), 2)]);
    assert_eq!(map.dirty_log_len(), 0);
}

#[test]
fn with_persisted_scan_propagates_persist_error_and_skips_scan() {
    let map = CacheLogMap::<String, usize>::new(CacheLogConfig::new(16, 16, 16));
    map.low_level().insert_dirty("a".to_owned(), 1);

    let mut scanned = false;
    let result = map.with_persisted_scan(
        1,
        |_batch| Err::<(), _>("persist-failed"),
        || {
            scanned = true;
            Ok::<_, &'static str>(())
        },
    );

    assert_eq!(result, Err("persist-failed"));
    assert!(!scanned, "scan should not run after persist failure");
    assert_eq!(map.dirty_log_len(), 1);
}

#[test]
fn coalesced_with_persisted_scan_uses_common_key_value_persist_surface() {
    let cfg = CacheLogConfig::new(32, 32, 32).with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
    let map = CacheLogMap::<String, usize>::new(cfg);
    map.put("dup".to_owned(), 1);
    map.put("keep".to_owned(), 2);
    map.put("dup".to_owned(), 3);

    let mut persisted = Vec::new();
    let result = map
        .with_persisted_scan(
            16,
            |batch| {
                persisted = batch
                    .iter()
                    .map(|entry| (entry.key().clone(), *entry.value()))
                    .collect();
                Ok::<_, &'static str>(())
            },
            || {
                assert_eq!(map.dirty_log_len(), 0);
                Ok::<_, &'static str>("scan-ok")
            },
        )
        .unwrap();

    persisted.sort();
    assert_eq!(result, "scan-ok");
    assert_eq!(
        persisted,
        vec![("dup".to_owned(), 3), ("keep".to_owned(), 2)]
    );
    assert!(map.read("dup", |_, value, state| (*value, state)).is_none());
    assert!(
        map.read("keep", |_, value, state| (*value, state))
            .is_none()
    );
}

#[test]
fn coalesced_persist_failure_keeps_snapshot_visible_and_retryable() {
    let cfg = CacheLogConfig::new(32, 32, 32).with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
    let map = CacheLogMap::<String, usize>::new(cfg);
    map.put("older".to_owned(), 1);
    map.put("later".to_owned(), 2);

    let result = map.with_flush_batch(1, |_batch| Err::<(), _>("persist-failed"));
    assert_eq!(result, Err("persist-failed"));

    assert_eq!(
        map.read("older", |_, value, state| (*value, state)),
        Some((1, EntryState::Dirty))
    );
    assert_eq!(
        map.read("later", |_, value, state| (*value, state)),
        Some((2, EntryState::Dirty))
    );
    assert_eq!(map.dirty_log_len(), 2);

    let retry = map.low_level().flush_batch(2);
    assert_eq!(retry.len(), 1);
}

#[test]
fn strict_batch_and_single_writes_flush_in_global_id_order() {
    let map = Arc::new(CacheLogMap::<Vec<u8>, Vec<u8>>::new(CacheLogConfig::new(
        32_768, 32_768, 16,
    )));

    let batch_map = Arc::clone(&map);
    let batch_writer = thread::spawn(move || {
        let entries: Vec<(Vec<u8>, Vec<u8>)> = (0..4096_u32)
            .map(|i| (format!("batch:{i:04}").into_bytes(), vec![0]))
            .collect();
        let borrowed: Vec<(&[u8], &[u8])> = entries
            .iter()
            .map(|(key, value)| (key.as_slice(), value.as_slice()))
            .collect();
        batch_map
            .low_level()
            .insert_dirty_batch_borrowed_without_ids(borrowed)
    });

    std::thread::sleep(std::time::Duration::from_millis(1));

    let single_map = Arc::clone(&map);
    let single_writer = thread::spawn(move || {
        for i in 0..256_u32 {
            let _ = single_map
                .low_level()
                .insert_dirty(format!("single:{i:04}").into_bytes(), vec![1]);
        }
    });

    let batch_count = batch_writer.join().expect("batch writer join");
    single_writer.join().expect("single writer join");

    let mut flushed_ids = Vec::new();
    loop {
        let batch = map.low_level().flush_batch(512);
        if batch.is_empty() {
            break;
        }
        flushed_ids.extend(batch.iter().map(|entry| entry.id));
        assert_eq!(map.low_level().mark_flushed(&batch), batch.len());
    }

    let expected: Vec<u64> = (0..(batch_count + 256) as u64).collect();
    assert_eq!(flushed_ids, expected);
}

#[test]
fn flush_now_drains_all_batches() {
    let map = CacheLogMap::<String, usize>::new(CacheLogConfig::new(16, 16, 16));
    map.low_level().insert_dirty("a".to_owned(), 1);
    map.low_level().insert_dirty("b".to_owned(), 2);
    map.low_level().insert_dirty("c".to_owned(), 3);

    let mut flushed = Vec::new();
    let committed = map
        .flush_now(2, |batch| {
            flushed.push(
                batch
                    .iter()
                    .map(|entry| entry.key().clone())
                    .collect::<Vec<_>>(),
            );
            Ok::<_, ()>(())
        })
        .unwrap();

    assert_eq!(committed, 3);
    assert_eq!(
        flushed,
        vec![vec!["a".to_owned(), "b".to_owned()], vec!["c".to_owned()]]
    );
    assert_eq!(map.dirty_log_len(), 0);
}

#[test]
fn flushing_old_dirty_does_not_clear_newer_visible_dirty() {
    let map = CacheLogMap::<String, usize>::new(CacheLogConfig::new(16, 16, 16));

    let _old_id = map.low_level().insert_dirty("same".to_owned(), 1);
    let new_id = map.low_level().insert_dirty("same".to_owned(), 2);

    let batch = map.low_level().flush_batch(1);
    assert_eq!(batch.len(), 1);
    assert_eq!(map.low_level().mark_flushed(&batch), 1);
    drop(batch);

    assert_eq!(
        read_triplet(&map, &"same".to_owned()),
        Some((2, EntryState::Dirty, VisibleRef::Dirty(new_id)))
    );
    assert_eq!(map.dirty_log_len(), 1);
}

#[test]
fn mark_flushed_does_not_remove_newer_visible_dirty() {
    let map = CacheLogMap::<String, usize>::new(CacheLogConfig::new(16, 16, 16));

    map.low_level().insert_dirty("same".to_owned(), 1);
    let batch = map.low_level().flush_batch(1);
    let newer = map.low_level().insert_dirty("same".to_owned(), 2);

    assert_eq!(map.low_level().mark_flushed(&batch), 1);
    assert_eq!(
        read_triplet(&map, &"same".to_owned()),
        Some((2, EntryState::Dirty, VisibleRef::Dirty(newer)))
    );
}

#[test]
fn flush_batch_reuses_inflight_batch_until_marked_flushed() {
    let map = CacheLogMap::<String, usize>::new(CacheLogConfig::new(16, 16, 16));

    map.low_level().insert_dirty("a".to_owned(), 1);
    map.low_level().insert_dirty("b".to_owned(), 2);

    let first = map.low_level().flush_batch(1);
    let retry = map.low_level().flush_batch(2);

    assert_eq!(first.len(), 1);
    assert_eq!(retry.len(), 1);
    assert_eq!(first.last_id(), retry.last_id());
    assert_eq!(map.dirty_log_len(), 2);

    assert_eq!(map.low_level().mark_flushed(&first), 1);
    let next = map.low_level().flush_batch(1);
    assert_eq!(next.len(), 1);
}

#[test]
fn evicting_clean_entry_removes_visible_clean_ref_immediately() {
    let map = CacheLogMap::<String, usize>::new(CacheLogConfig::new(16, 16, 16));

    assert_eq!(map.insert_clean_if_absent("clean".to_owned(), 5), Some(0));
    assert!(map.evict_clean(&"clean".to_owned()));

    assert_eq!(map.read(&"clean".to_owned(), |_, _, _| ()), None);
    assert_eq!(map.low_level().visible_ref(&"clean".to_owned()), None);
    assert_eq!(map.clean_store_len(), 0);
}

#[test]
fn concurrent_readers_and_writer_smoke() {
    let map = Arc::new(CacheLogMap::<usize, usize>::new(CacheLogConfig::new(
        1024, 1024, 1024,
    )));
    let started = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let published = Arc::new(AtomicUsize::new(0));

    let writer_map = map.clone();
    let writer_started = started.clone();
    let writer_done = done.clone();
    let writer_published = published.clone();
    let writer = thread::spawn(move || {
        while !writer_started.load(Ordering::Acquire) {
            thread::yield_now();
        }
        for i in 0..256usize {
            writer_map.put(i, i * 10);
            writer_published.store(i + 1, Ordering::Release);
        }
        writer_done.store(true, Ordering::Release);
    });

    let mut readers = Vec::new();
    for _ in 0..3 {
        let reader_map = map.clone();
        let reader_started = started.clone();
        let reader_done = done.clone();
        let reader_published = published.clone();
        readers.push(thread::spawn(move || {
            while !reader_started.load(Ordering::Acquire) {
                thread::yield_now();
            }
            while !reader_done.load(Ordering::Acquire) {
                let upto = reader_published.load(Ordering::Acquire);
                for key in 0..upto {
                    if let Some((value, state)) =
                        reader_map.read(&key, |_, value, state| (*value, state))
                    {
                        assert_eq!(value, key * 10);
                        assert_eq!(state, EntryState::Dirty);
                    }
                }
            }
        }));
    }

    started.store(true, Ordering::Release);
    writer.join().unwrap();
    for reader in readers {
        reader.join().unwrap();
    }

    for key in 0..256usize {
        assert_eq!(read_pair(&map, &key), Some((key * 10, EntryState::Dirty)));
    }
}

// Common product coverage for BytePrefixMap: prefix behavior and trie
// maintenance observed through product-facing results rather than dirty ids.

#[test]
fn byte_prefix_map_requires_advance_trie_to_surface_prefixes() {
    let map = BytePrefixMap::<usize>::new(CacheLogConfig::new(64, 64, 64));
    map.insert_dirty(b"ab:001".to_vec(), 1);
    map.insert_dirty(b"ab:002".to_vec(), 2);
    map.insert_dirty(b"zz:001".to_vec(), 3);

    let before = map.list_prefix(b"ab:", |_, value, _| *value, 10);
    assert!(before.is_empty());

    let applied = map.advance_trie(64);
    assert!(applied >= 2);

    let mut rows = map.list_prefix(b"ab:", |_, value, _| *value, 10);
    rows.sort_unstable();
    assert_eq!(rows, vec![1, 2]);
}

#[test]
fn byte_prefix_map_for_each_prefix_key_filters_stale_keys() {
    let map = BytePrefixMap::<usize>::new(CacheLogConfig::new(64, 64, 64));
    map.insert_dirty(b"ab:001".to_vec(), 1);
    map.insert_dirty(b"ab:002".to_vec(), 2);
    let _ = map.advance_trie(64);

    let batch = map.flush_batch(2);
    assert_eq!(map.mark_flushed(&batch), 2);

    let mut keys = Vec::new();
    let matched = map.for_each_prefix_key(
        b"ab:",
        |k| keys.push(String::from_utf8_lossy(k).into_owned()),
        10,
    );
    assert_eq!(matched, 0);
    assert!(keys.is_empty());
}

#[test]
fn byte_prefix_map_advance_trie_dedupes_batched_updates() {
    let map = BytePrefixMap::<usize>::new(CacheLogConfig::new(64, 64, 64));
    map.insert_dirty(b"ab:001".to_vec(), 1);
    map.insert_dirty(b"ab:001".to_vec(), 2);
    map.insert_dirty(b"ab:002".to_vec(), 3);

    let applied = map.advance_trie(64);
    assert_eq!(applied, 2);

    let mut rows = map.list_prefix(b"ab:", |_, value, _| *value, 10);
    rows.sort_unstable();
    assert_eq!(rows, vec![2, 3]);
}

#[test]
fn byte_prefix_map_mark_flushed_keeps_rewritten_key_in_trie() {
    let map = BytePrefixMap::<usize>::new(CacheLogConfig::new(64, 64, 64));

    map.insert_dirty(b"ab:001".to_vec(), 1);
    let _ = map.advance_trie(64);
    let first = map.flush_batch(1);
    assert_eq!(first.len(), 1);

    map.insert_dirty(b"ab:001".to_vec(), 2);
    let _ = map.advance_trie(64);

    assert_eq!(map.mark_flushed(&first), 1);
    assert_eq!(map.list_prefix(b"ab:", |_, value, _| *value, 10), vec![2]);
}

// Strict/coalesced mode-specific coverage: behavior that intentionally differs
// by dirty-write mode and is not generic product semantics.

#[test]
fn insert_dirty_batch_strict_keeps_all_writes_in_order() {
    let map = CacheLogMap::<String, usize>::new(CacheLogConfig::new(32, 32, 32));

    let ids = map.low_level().insert_dirty_batch(vec![
        ("same".to_owned(), 1),
        ("same".to_owned(), 2),
        ("same".to_owned(), 3),
    ]);

    assert_eq!(ids.len(), 3);
    let batch = map.low_level().flush_batch(10);
    let values = batch.iter().map(|entry| entry.value).collect::<Vec<_>>();
    assert_eq!(values, vec![1, 2, 3]);
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

fn run_dirty_order_case() {
    let map = CacheLogMap::<String, usize>::new(CacheLogConfig::new(64, 64, 64));

    let ids = map.low_level().insert_dirty_batch(vec![
        ("k1".to_owned(), 1),
        ("k2".to_owned(), 2),
        ("k3".to_owned(), 3),
    ]);
    assert_eq!(ids.len(), 3);

    let batch = map.low_level().flush_batch(10);
    assert_eq!(batch.len(), 3);
    let rows = batch
        .iter()
        .map(|entry| (entry.id, entry.key.clone(), entry.value))
        .collect::<Vec<_>>();

    assert_eq!(rows[0].1, "k1");
    assert_eq!(rows[1].1, "k2");
    assert_eq!(rows[2].1, "k3");
    assert_eq!(map.low_level().mark_flushed(&batch), 3);
    assert_eq!(map.dirty_log_len(), 0);
}

#[test]
fn dirty_queue_preserves_order_and_flush() {
    run_dirty_order_case();
}

#[test]
fn strict_batch_order_is_preserved() {
    let cfg = CacheLogConfig::new(16, 16, 16).with_dirty_write_mode(DirtyWriteMode::StrictLog);
    let map = CacheLogMap::<String, usize>::new(cfg);

    let _ = map.low_level().insert_dirty_batch(vec![
        ("k1".to_owned(), 1),
        ("k2".to_owned(), 2),
        ("k3".to_owned(), 3),
        ("k4".to_owned(), 4),
    ]);

    let batch = map.low_level().flush_batch(8);
    let values = batch.iter().map(|entry| entry.value).collect::<Vec<_>>();
    assert_eq!(values, vec![1, 2, 3, 4]);
    assert_eq!(map.low_level().mark_flushed(&batch), 4);
    assert_eq!(map.dirty_log_len(), 0);
}

fn run_dirty_concurrent_flush_case() {
    let map = Arc::new(CacheLogMap::<usize, usize>::new(CacheLogConfig::new(
        2048, 2048, 64,
    )));

    let writer_count = 4usize;
    let per_writer = 250usize;
    let total = writer_count * per_writer;
    let done = Arc::new(AtomicBool::new(false));
    let flushed = Arc::new(AtomicUsize::new(0));

    let mut writers = Vec::new();
    for w in 0..writer_count {
        let map = map.clone();
        writers.push(thread::spawn(move || {
            let base = w * per_writer;
            for i in 0..per_writer {
                let k = base + i;
                let _ = map.low_level().insert_dirty(k, k);
            }
        }));
    }

    let map_f = map.clone();
    let done_f = done.clone();
    let flushed_f = flushed.clone();
    let flusher = thread::spawn(move || {
        loop {
            let batch = map_f.low_level().flush_batch(64);
            if batch.is_empty() {
                if done_f.load(Ordering::Acquire) {
                    break;
                }
                thread::yield_now();
                continue;
            }
            let n = map_f.low_level().mark_flushed(&batch);
            flushed_f.fetch_add(n, Ordering::AcqRel);
        }
    });

    for w in writers {
        w.join().expect("writer join");
    }
    done.store(true, Ordering::Release);
    flusher.join().expect("flusher join");

    loop {
        let batch = map.low_level().flush_batch(64);
        if batch.is_empty() {
            break;
        }
        let n = map.low_level().mark_flushed(&batch);
        flushed.fetch_add(n, Ordering::AcqRel);
    }

    let flushed_count = flushed.load(Ordering::Acquire);
    let dirty_len = map.dirty_log_len();
    let visible_len = map.visible_len();
    let mut leftovers = Vec::new();
    for key in 0..total {
        if let Some((value, state, visible)) =
            map.low_level().read_full(&key, |_, v, s, vr| (*v, s, vr))
        {
            leftovers.push((key, value, state, visible));
            if leftovers.len() >= 16 {
                break;
            }
        }
    }
    assert_eq!(
        flushed_count, total,
        "flushed={flushed_count} total={total}"
    );
    assert_eq!(dirty_len, 0, "dirty_len={dirty_len}");
    assert_eq!(
        visible_len, 0,
        "visible_len={visible_len} leftovers={leftovers:?}"
    );
}

#[test]
fn dirty_queue_concurrent_flushes_all() {
    run_dirty_concurrent_flush_case();
}

// Coalesced-specific coverage: latest-value collapse and borrowed/owned parity.

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
fn wait_flush_work_blocks_and_returns_batch_when_data_arrives() {
    let map = Arc::new(CacheLogMap::<u64, u64>::new(CacheLogConfig::new(
        64, 64, 64,
    )));
    let map_wait = Arc::clone(&map);

    let waiter = thread::spawn(move || map_wait.low_level().wait_flush_work(16));

    // Let the waiter park on queue recv.
    thread::sleep(std::time::Duration::from_millis(5));
    let id = map.low_level().insert_dirty(7, 77);

    let work = waiter.join().expect("waiter join");
    match work {
        FlushWork::Batch(batch) => {
            assert_eq!(batch.len(), 1);
            let record = batch.iter().next().expect("batch record");
            assert_eq!(record.id, id);
            assert_eq!(record.key, 7);
            assert_eq!(record.value, 77);
        }
        FlushWork::ForceFlush => panic!("expected dirty batch, got force flush"),
    }
}

#[cfg(not(feature = "loom"))]
#[test]
fn background_flush_config_for_batch_size_matches_integration_policy() {
    use cachelog::BackgroundFlushConfig;

    let small = BackgroundFlushConfig::for_batch_size(64);
    assert_eq!(small.trigger_dirty, 64);
    assert_eq!(small.flush_limit, 512);
    assert_eq!(small.check_interval, std::time::Duration::from_millis(100));

    let large = BackgroundFlushConfig::for_batch_size(256);
    assert_eq!(large.trigger_dirty, 256);
    assert_eq!(large.flush_limit, 512);
    assert_eq!(large.check_interval, std::time::Duration::from_secs(10));
}

#[cfg(not(feature = "loom"))]
#[test]
fn background_flush_default_entry_point_flushes_pending_writes() {
    use std::sync::Mutex;

    let map = Arc::new(CacheLogMap::<u64, u64>::new(CacheLogConfig::new(
        128, 128, 16,
    )));
    map.low_level().insert_dirty(1, 11);
    map.low_level().insert_dirty(2, 22);

    let persisted = Arc::new(Mutex::new(Vec::new()));
    let persisted_flush = Arc::clone(&persisted);
    let service = map.start_background_flush_default(move |batch| {
        persisted_flush
            .lock()
            .expect("persisted lock")
            .extend(batch.iter().map(|entry| (*entry.key(), *entry.value())));
        Ok(())
    });

    service.note_writes(2).expect("wake worker");
    service.flush_sync().expect("flush sync");

    let mut rows = persisted.lock().expect("persisted lock").clone();
    rows.sort_unstable();
    assert_eq!(rows, vec![(1, 11), (2, 22)]);
}

#[cfg(not(feature = "loom"))]
#[test]
fn background_flush_batch_size_entry_point_flushes_pending_writes() {
    use std::sync::Mutex;

    let map = Arc::new(CacheLogMap::<u64, u64>::new(CacheLogConfig::new(
        128, 128, 16,
    )));
    map.low_level().insert_dirty(1, 11);
    map.low_level().insert_dirty(2, 22);

    let persisted = Arc::new(Mutex::new(Vec::new()));
    let persisted_flush = Arc::clone(&persisted);
    let service = map.start_background_flush_for_batch_size(2, move |batch| {
        persisted_flush
            .lock()
            .expect("persisted lock")
            .extend(batch.iter().map(|entry| (*entry.key(), *entry.value())));
        Ok(())
    });

    service.note_writes(2).expect("wake worker");
    service.flush_sync().expect("flush sync");

    let mut rows = persisted.lock().expect("persisted lock").clone();
    rows.sort_unstable();
    assert_eq!(rows, vec![(1, 11), (2, 22)]);
}

#[cfg(not(feature = "loom"))]
#[test]
fn background_flush_persist_error_stops_service() {
    use cachelog::BackgroundFlushConfig;

    let map = Arc::new(CacheLogMap::<u64, u64>::new(CacheLogConfig::new(
        64, 64, 64,
    )));
    map.low_level().insert_dirty(1, 11);

    let service = map.start_background_flush(
        BackgroundFlushConfig::new(1, 16, std::time::Duration::from_millis(100)),
        |_batch| Err("persist failed".to_string()),
    );

    service.note_writes(1).expect("wake worker");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while service.is_running() && std::time::Instant::now() < deadline {
        thread::sleep(std::time::Duration::from_millis(5));
    }

    assert!(
        !service.is_running(),
        "service kept running after persist error"
    );
    let result = service.flush_sync();
    assert!(
        result
            .as_ref()
            .is_err_and(|err| err.contains("persist failed")),
        "unexpected flush_sync result after persist failure: {result:?}"
    );
}

#[cfg(not(feature = "loom"))]
#[test]
fn background_flush_respects_flush_limit_per_persist_batch() {
    use cachelog::BackgroundFlushConfig;
    use std::sync::Mutex;

    let map = Arc::new(CacheLogMap::<u64, u64>::new(CacheLogConfig::new(
        64, 64, 64,
    )));
    map.low_level().insert_dirty(1, 11);
    map.low_level().insert_dirty(2, 22);
    map.low_level().insert_dirty(3, 33);

    let persisted = Arc::new(Mutex::new(Vec::new()));
    let persisted_flush = Arc::clone(&persisted);
    let service = map.start_background_flush(
        BackgroundFlushConfig::new(1, 2, std::time::Duration::from_millis(100)),
        move |batch| {
            persisted_flush
                .lock()
                .expect("persisted lock")
                .push(batch.len());
            Ok(())
        },
    );

    service.note_writes(3).expect("wake worker");
    service.flush_sync().expect("flush sync");

    let lens = persisted.lock().expect("persisted lock").clone();
    assert_eq!(lens.iter().sum::<usize>(), 3, "persisted lens: {lens:?}");
    assert!(
        lens.iter().all(|&len| len <= 2),
        "flush_limit violated, batches: {lens:?}"
    );
}

#[cfg(not(feature = "loom"))]
#[test]
fn coalesced_background_flush_drains_backlog_below_trigger() {
    use cachelog::BackgroundFlushConfig;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    let cfg =
        CacheLogConfig::new(2048, 2048, 2048).with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
    let map = Arc::new(CacheLogMap::<u64, u64>::new(cfg));
    let persisted = Arc::new(Mutex::new(Vec::new()));
    let persisted_flush = Arc::clone(&persisted);
    let service = map.start_background_flush(
        BackgroundFlushConfig::new(1024, 512, Duration::from_millis(100)),
        move |batch| {
            persisted_flush
                .lock()
                .expect("persisted lock")
                .push(batch.len());
            Ok(())
        },
    );

    for i in 0..1024_u64 {
        map.low_level().insert_dirty(i, i);
    }
    service.note_writes(1024).expect("wake worker");

    let deadline = Instant::now() + Duration::from_secs(1);
    while map.dirty_log_len() != 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }

    let lens = persisted.lock().expect("persisted lock").clone();
    assert_eq!(lens.iter().sum::<usize>(), 1024, "persisted lens: {lens:?}");
    assert_eq!(map.dirty_log_len(), 0);
}

#[cfg(not(feature = "loom"))]
#[test]
fn background_flush_request_flush_drains_without_note_writes() {
    use cachelog::BackgroundFlushConfig;
    use std::sync::Mutex;

    let map = Arc::new(CacheLogMap::<u64, u64>::new(CacheLogConfig::new(
        64, 64, 64,
    )));
    let persisted = Arc::new(Mutex::new(Vec::new()));
    let persisted_flush = Arc::clone(&persisted);
    let service = map.start_background_flush(
        BackgroundFlushConfig::new(100, 4, std::time::Duration::from_millis(100)),
        move |batch| {
            persisted_flush
                .lock()
                .expect("persisted lock")
                .push(batch.len());
            Ok(())
        },
    );

    map.low_level().insert_dirty(1, 11);
    service.request_flush().expect("request flush");
    service.flush_sync().expect("flush sync");
    assert_eq!(map.dirty_log_len(), 0);

    map.low_level().insert_dirty(2, 22);
    service.request_flush().expect("request flush again");
    service.flush_sync().expect("flush sync again");
    assert_eq!(map.dirty_log_len(), 0);

    let lens = persisted.lock().expect("persisted lock").clone();
    assert_eq!(lens.iter().sum::<usize>(), 2, "persisted lens: {lens:?}");
}

#[cfg(not(feature = "loom"))]
#[test]
fn background_flush_check_interval_flushes_without_note_writes() {
    use cachelog::BackgroundFlushConfig;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    let map = Arc::new(CacheLogMap::<u64, u64>::new(CacheLogConfig::new(
        64, 64, 64,
    )));
    let persisted = Arc::new(Mutex::new(Vec::new()));
    let persisted_flush = Arc::clone(&persisted);
    let service = map.start_background_flush(
        BackgroundFlushConfig::new(1, 4, Duration::from_millis(10)),
        move |batch| {
            persisted_flush
                .lock()
                .expect("persisted lock")
                .push(batch.len());
            Ok(())
        },
    );

    map.low_level().insert_dirty(1, 11);

    let deadline = Instant::now() + Duration::from_secs(1);
    while map.dirty_log_len() != 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }

    assert_eq!(map.dirty_log_len(), 0, "background flusher never polled");
    let lens = persisted.lock().expect("persisted lock").clone();
    assert_eq!(lens.iter().sum::<usize>(), 1, "persisted lens: {lens:?}");
    service.shutdown().expect("shutdown");
}

#[cfg(not(feature = "loom"))]
#[test]
fn background_flush_reconfigure_lower_threshold_wakes_existing_backlog() {
    use cachelog::BackgroundFlushConfig;
    use std::sync::Mutex;

    let map = Arc::new(CacheLogMap::<u64, u64>::new(CacheLogConfig::new(
        64, 64, 64,
    )));
    for i in 0..10_u64 {
        map.low_level().insert_dirty(i, i);
    }

    let persisted = Arc::new(Mutex::new(Vec::new()));
    let persisted_flush = Arc::clone(&persisted);
    let service = map.start_background_flush(
        BackgroundFlushConfig::new(100, 4, std::time::Duration::from_millis(100)),
        move |batch| {
            persisted_flush
                .lock()
                .expect("persisted lock")
                .push(batch.len());
            Ok(())
        },
    );

    service
        .reconfigure(3, 4)
        .expect("reconfigure should succeed");
    service.flush_sync().expect("flush sync");

    let lens = persisted.lock().expect("persisted lock").clone();
    assert!(
        !lens.is_empty(),
        "reconfigure should have woken the worker for existing backlog"
    );
    assert_eq!(lens.iter().sum::<usize>(), 10, "persisted lens: {lens:?}");
}

#[cfg(not(feature = "loom"))]
#[test]
fn background_flush_reconfigure_for_batch_size_wakes_existing_backlog() {
    use cachelog::BackgroundFlushConfig;
    use std::sync::Mutex;

    let map = Arc::new(CacheLogMap::<u64, u64>::new(CacheLogConfig::new(
        64, 64, 64,
    )));
    for i in 0..10_u64 {
        map.low_level().insert_dirty(i, i);
    }

    let persisted = Arc::new(Mutex::new(Vec::new()));
    let persisted_flush = Arc::clone(&persisted);
    let service = map.start_background_flush(
        BackgroundFlushConfig::new(100, 4, std::time::Duration::from_millis(100)),
        move |batch| {
            persisted_flush
                .lock()
                .expect("persisted lock")
                .push(batch.len());
            Ok(())
        },
    );

    service
        .reconfigure_for_batch_size(3)
        .expect("reconfigure should succeed");
    service.flush_sync().expect("flush sync");

    let lens = persisted.lock().expect("persisted lock").clone();
    assert!(
        !lens.is_empty(),
        "reconfigure_for_batch_size should have woken the worker for existing backlog"
    );
    assert_eq!(lens.iter().sum::<usize>(), 10, "persisted lens: {lens:?}");
}

// Background flush integration coverage: common persistence service behavior
// independent of strict dirty-id ordering.

#[cfg(not(feature = "loom"))]
#[test]
fn background_flush_shutdown_unblocks_cloned_handle_flush_sync() {
    use cachelog::BackgroundFlushConfig;
    use std::sync::mpsc;

    let map = Arc::new(CacheLogMap::<u64, u64>::new(CacheLogConfig::new(
        64, 64, 64,
    )));
    map.low_level().insert_dirty(1, 11);

    let service = Arc::new(map.start_background_flush(
        BackgroundFlushConfig::new(1, 16, std::time::Duration::from_millis(100)),
        |_batch| Ok(()),
    ));
    let clone = Arc::clone(&service);

    let (tx, rx) = mpsc::channel();
    let waiter = thread::spawn(move || {
        let result = clone.flush_sync();
        tx.send(result).expect("send flush result");
    });

    let shutdown = service.shutdown();
    assert!(shutdown.is_ok(), "shutdown failed: {shutdown:?}");

    let result = rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("flush_sync did not return after shutdown");
    assert!(
        result
            .as_ref()
            .is_err_and(|err| err.contains("shutting down")),
        "unexpected flush_sync result after shutdown: {result:?}"
    );
    waiter.join().expect("waiter join");
}

#[test]
fn borrowed_batch_insert_reserves_unique_ids_under_concurrency() {
    use std::sync::Barrier;

    let map = Arc::new(CacheLogMap::<Vec<u8>, Vec<u8>>::new(CacheLogConfig::new(
        16_384, 16_384, 16,
    )));
    let start = Arc::new(Barrier::new(3));

    let map_batch = Arc::clone(&map);
    let start_batch = Arc::clone(&start);
    let batch_writer = thread::spawn(move || {
        start_batch.wait();
        for i in 0..512_u32 {
            let key1 = format!("batch-a-{i}");
            let val1 = format!("value-a-{i}");
            let key2 = format!("batch-b-{i}");
            let val2 = format!("value-b-{i}");
            map_batch
                .low_level()
                .insert_dirty_batch_borrowed_without_ids(
                    [
                        (key1.as_bytes(), val1.as_bytes()),
                        (key2.as_bytes(), val2.as_bytes()),
                    ]
                    .into_iter(),
                );
        }
    });

    let map_single = Arc::clone(&map);
    let start_single = Arc::clone(&start);
    let single_writer = thread::spawn(move || {
        start_single.wait();
        for i in 0..1024_u32 {
            map_single.low_level().insert_dirty(
                format!("single-{i}").into_bytes(),
                format!("single-value-{i}").into_bytes(),
            );
        }
    });

    start.wait();
    batch_writer.join().expect("batch writer join");
    single_writer.join().expect("single writer join");

    let mut ids = Vec::new();
    loop {
        let batch = map.low_level().flush_batch(4096);
        if batch.is_empty() {
            break;
        }
        ids.extend(batch.iter().map(|entry| entry.id));
        assert_eq!(map.low_level().mark_flushed(&batch), batch.len());
    }

    let expected = 2048_usize;
    assert_eq!(ids.len(), expected, "flushed ids len mismatch");
    let unique = ids.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(unique.len(), expected, "duplicate ids observed");
    assert_eq!(unique.first().copied(), Some(0));
    assert_eq!(unique.last().copied(), Some(expected as u64 - 1));
}

#[test]
fn borrowed_coalesced_batch_matches_owned_duplicate_semantics() {
    let config =
        CacheLogConfig::new(64, 64, 64).with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
    let owned = CacheLogMap::<Vec<u8>, Vec<u8>>::new(config);
    let borrowed = CacheLogMap::<Vec<u8>, Vec<u8>>::new(config);

    let owned_count = owned.low_level().insert_dirty_batch_without_ids(vec![
        (b"dup".to_vec(), vec![1]),
        (b"keep".to_vec(), vec![2]),
        (b"dup".to_vec(), vec![3]),
        (b"dup".to_vec(), vec![4]),
    ]);

    let borrowed_entries = [
        (&b"dup"[..], &b"\x01"[..]),
        (&b"keep"[..], &b"\x02"[..]),
        (&b"dup"[..], &b"\x03"[..]),
        (&b"dup"[..], &b"\x04"[..]),
    ];
    let borrowed_count = borrowed
        .low_level()
        .insert_dirty_batch_borrowed_without_ids(borrowed_entries);

    assert_eq!(owned_count, 2);
    assert_eq!(borrowed_count, owned_count);
    assert_eq!(owned.dirty_log_len(), 2);
    assert_eq!(borrowed.dirty_log_len(), 2);
    for (key, value) in [(b"dup".as_slice(), vec![4]), (b"keep".as_slice(), vec![2])] {
        assert_eq!(borrowed.read(key, |_, value, _| value.clone()), Some(value));
    }
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

#[test]
fn force_flush_signal_wakes_waiter_without_dirty_data() {
    let map = Arc::new(CacheLogMap::<u64, u64>::new(CacheLogConfig::new(
        64, 64, 64,
    )));
    let map_wait = Arc::clone(&map);

    let waiter = thread::spawn(move || map_wait.low_level().wait_flush_work(16));

    thread::sleep(std::time::Duration::from_millis(5));
    map.low_level().signal_force_flush();

    let work = waiter.join().expect("waiter join");
    assert!(matches!(work, FlushWork::ForceFlush));
}
