#![cfg(not(feature = "loom"))]

use std::borrow::Borrow;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;

use cachelog::{CacheLogConfig, CacheLogMap, EntryState, VisibleRef};

fn read_triplet<K, Q, V>(map: &CacheLogMap<K, V>, key: &Q) -> Option<(V, EntryState, VisibleRef)>
where
    K: Borrow<Q> + Clone + Eq + std::hash::Hash,
    Q: Eq + std::hash::Hash + ?Sized,
    V: Copy,
{
    map.read(key, |_, value, state, visible| (*value, state, visible))
}

#[test]
fn dirty_write_is_immediately_visible() {
    let map = CacheLogMap::<String, usize>::new(CacheLogConfig::new(16, 16, 16));

    let id = map.insert_dirty("alpha".to_owned(), 1);

    assert!(matches!(id, _));
    assert_eq!(
        read_triplet(&map, &"alpha".to_owned()),
        Some((1, EntryState::Dirty, VisibleRef::Dirty(id)))
    );
}

#[test]
fn borrowed_string_lookup_works_across_public_api() {
    let map = CacheLogMap::<String, usize>::new(CacheLogConfig::new(16, 16, 16));
    let id = map.insert_dirty("borrowed".to_owned(), 9);

    assert!(map.contains("borrowed"));
    assert_eq!(map.visible_ref("borrowed"), Some(VisibleRef::Dirty(id)));
    assert_eq!(
        map.get_cloned("borrowed"),
        Some((9, EntryState::Dirty, VisibleRef::Dirty(id)))
    );
    assert_eq!(
        map.read("borrowed", |key, value, state, visible| {
            (key.clone(), *value, state, visible)
        }),
        Some((
            "borrowed".to_owned(),
            9,
            EntryState::Dirty,
            VisibleRef::Dirty(id)
        ))
    );
    assert!(map.cleanup_stale_visible("borrowed"));
    assert!(!map.contains("borrowed"));
}

#[test]
fn borrowed_bytes_lookup_works_across_public_api() {
    let map = CacheLogMap::<Vec<u8>, usize>::new(CacheLogConfig::new(16, 16, 16));
    let id = map.insert_dirty(b"bytes".to_vec(), 11);

    assert!(map.contains(b"bytes".as_slice()));
    assert_eq!(
        map.visible_ref(b"bytes".as_slice()),
        Some(VisibleRef::Dirty(id))
    );
    assert_eq!(
        map.get_cloned(b"bytes".as_slice()),
        Some((11, EntryState::Dirty, VisibleRef::Dirty(id)))
    );
    assert_eq!(
        map.read(b"bytes".as_slice(), |key, value, state, visible| {
            (key.clone(), *value, state, visible)
        }),
        Some((
            b"bytes".to_vec(),
            11,
            EntryState::Dirty,
            VisibleRef::Dirty(id)
        ))
    );
    assert!(map.cleanup_stale_visible(b"bytes".as_slice()));
    assert!(!map.contains(b"bytes".as_slice()));
}

#[test]
fn list_prefix_scans_visible_entries_without_flush() {
    let map = CacheLogMap::<Vec<u8>, usize>::new(CacheLogConfig::new(64, 64, 64));
    map.insert_dirty(b"ab:1".to_vec(), 1);
    map.insert_dirty(b"ab:2".to_vec(), 2);
    map.insert_dirty(b"ac:1".to_vec(), 3);
    assert_eq!(map.insert_clean_if_absent(b"ab:clean".to_vec(), 4), Some(0));

    let rows = map.list_prefix(
        b"ab:",
        |key, value, state, visible| (key.clone(), *value, state, visible),
        16,
    );
    assert_eq!(rows.len(), 3);

    let keys = rows
        .into_iter()
        .map(|(k, _, _, _)| k)
        .collect::<BTreeSet<_>>();
    assert!(keys.contains(b"ab:1".as_slice()));
    assert!(keys.contains(b"ab:2".as_slice()));
    assert!(keys.contains(b"ab:clean".as_slice()));
}

#[test]
fn list_prefix_respects_limit() {
    let map = CacheLogMap::<Vec<u8>, usize>::new(CacheLogConfig::new(64, 64, 64));
    map.insert_dirty(b"ab:1".to_vec(), 1);
    map.insert_dirty(b"ab:2".to_vec(), 2);
    map.insert_dirty(b"ab:3".to_vec(), 3);

    let rows = map.list_prefix(b"ab:", |_, value, _, _| *value, 2);
    assert_eq!(rows.len(), 2);
}

#[test]
fn for_each_prefix_reports_match_count_without_allocating_results() {
    let map = CacheLogMap::<Vec<u8>, usize>::new(CacheLogConfig::new(64, 64, 64));
    map.insert_dirty(b"ab:1".to_vec(), 1);
    map.insert_dirty(b"ab:2".to_vec(), 2);
    map.insert_dirty(b"zz:1".to_vec(), 3);

    let mut sum = 0_usize;
    let matched = map.for_each_prefix(
        b"ab:",
        |_, value, state, _| {
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
    map.insert_dirty(b"ab:1".to_vec(), 1);
    map.insert_dirty(b"ab:2".to_vec(), 2);
    map.insert_dirty(b"ab:3".to_vec(), 3);

    let mut seen = 0_usize;
    let matched = map.for_each_prefix(
        b"ab:",
        |_, _, _, _| {
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
    map.insert_dirty(b"ab:1".to_vec(), 1);
    map.insert_dirty(b"ab:2".to_vec(), 2);
    map.insert_dirty(b"zz:1".to_vec(), 3);

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
fn clean_insert_requires_absence() {
    let map = CacheLogMap::<String, usize>::new(CacheLogConfig::new(16, 16, 16));

    assert_eq!(map.insert_clean_if_absent("beta".to_owned(), 7), Some(0));
    assert_eq!(map.insert_clean_if_absent("beta".to_owned(), 8), None);
    assert_eq!(
        read_triplet(&map, &"beta".to_owned()),
        Some((7, EntryState::Clean, VisibleRef::Clean(0)))
    );
}

#[test]
fn dirty_write_replaces_visible_clean_ref() {
    let map = CacheLogMap::<String, usize>::new(CacheLogConfig::new(16, 16, 16));

    assert_eq!(map.insert_clean_if_absent("gamma".to_owned(), 3), Some(0));
    let dirty_id = map.insert_dirty("gamma".to_owned(), 4);

    assert_eq!(
        read_triplet(&map, &"gamma".to_owned()),
        Some((4, EntryState::Dirty, VisibleRef::Dirty(dirty_id)))
    );
}

#[test]
fn flush_batch_is_in_write_order() {
    let map = CacheLogMap::<String, usize>::new(CacheLogConfig::new(16, 16, 16));

    map.insert_dirty("a".to_owned(), 1);
    map.insert_dirty("b".to_owned(), 2);
    map.insert_dirty("c".to_owned(), 3);

    let batch = map.flush_batch(2);
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

    map.insert_dirty("same".to_owned(), 1);
    map.insert_dirty("same".to_owned(), 2);

    let batch = map.flush_batch(2);
    assert_eq!(map.mark_flushed(&batch), 2);
    assert_eq!(map.dirty_log_len(), 0);
    drop(batch);
    assert_eq!(map.read(&"same".to_owned(), |_, _, _, _| ()), None);
    assert_eq!(map.visible_ref(&"same".to_owned()), None);
}

#[test]
fn flushing_old_dirty_does_not_clear_newer_visible_dirty() {
    let map = CacheLogMap::<String, usize>::new(CacheLogConfig::new(16, 16, 16));

    let _old_id = map.insert_dirty("same".to_owned(), 1);
    let new_id = map.insert_dirty("same".to_owned(), 2);

    let batch = map.flush_batch(1);
    assert_eq!(batch.len(), 1);
    assert_eq!(map.mark_flushed(&batch), 1);
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

    map.insert_dirty("same".to_owned(), 1);
    let batch = map.flush_batch(1);
    let newer = map.insert_dirty("same".to_owned(), 2);

    assert_eq!(map.mark_flushed(&batch), 1);
    assert_eq!(
        read_triplet(&map, &"same".to_owned()),
        Some((2, EntryState::Dirty, VisibleRef::Dirty(newer)))
    );
}

#[test]
fn flush_batch_reuses_inflight_batch_until_marked_flushed() {
    let map = CacheLogMap::<String, usize>::new(CacheLogConfig::new(16, 16, 16));

    map.insert_dirty("a".to_owned(), 1);
    map.insert_dirty("b".to_owned(), 2);

    let first = map.flush_batch(1);
    let retry = map.flush_batch(2);

    assert_eq!(first.len(), 1);
    assert_eq!(retry.len(), 1);
    assert_eq!(first.last_id(), retry.last_id());
    assert_eq!(map.dirty_log_len(), 2);

    assert_eq!(map.mark_flushed(&first), 1);
    let next = map.flush_batch(1);
    assert_eq!(next.len(), 1);
}

#[test]
fn evicting_clean_entry_removes_visible_clean_ref_immediately() {
    let map = CacheLogMap::<String, usize>::new(CacheLogConfig::new(16, 16, 16));

    assert_eq!(map.insert_clean_if_absent("clean".to_owned(), 5), Some(0));
    assert!(map.evict_clean(&"clean".to_owned()));

    assert_eq!(map.read(&"clean".to_owned(), |_, _, _, _| ()), None);
    assert_eq!(map.visible_ref(&"clean".to_owned()), None);
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
            writer_map.insert_dirty(i, i * 10);
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
                    if let Some((value, state, visible)) =
                        reader_map.read(&key, |_, value, state, visible| (*value, state, visible))
                    {
                        assert_eq!(value, key * 10);
                        assert_eq!(state, EntryState::Dirty);
                        assert!(matches!(visible, VisibleRef::Dirty(_)));
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
        let result = read_triplet(&map, &key);
        assert!(matches!(
            result,
            Some((value, EntryState::Dirty, VisibleRef::Dirty(_))) if value == key * 10
        ));
    }
}
