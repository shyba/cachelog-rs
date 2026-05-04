#![cfg(not(feature = "loom"))]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;

use bytes::Bytes;
use cachelog::{CacheLogConfig, CacheLogMap, DirtyWriteMode, EntryState};
use common::read_pair;

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
