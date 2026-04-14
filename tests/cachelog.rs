#![cfg(not(feature = "loom"))]

use std::borrow::Borrow;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;

use cachelog::{
    BytePrefixMap, CacheLogConfig, CacheLogMap, DirtyAllocMode, DirtyQueueBackend, DirtyWriteMode,
    EntryState, VisibleRef,
};

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
fn list_prefix_returns_keys_in_lexicographic_order() {
    let map = CacheLogMap::<Vec<u8>, usize>::new(CacheLogConfig::new(64, 64, 64));
    map.insert_dirty(b"ab:20".to_vec(), 20);
    map.insert_dirty(b"ab:03".to_vec(), 3);
    map.insert_dirty(b"ab:11".to_vec(), 11);

    let keys = map.list_prefix(b"ab:", |key, _, _, _| key.clone(), 16);
    assert_eq!(
        keys,
        vec![b"ab:03".to_vec(), b"ab:11".to_vec(), b"ab:20".to_vec()]
    );
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

#[test]
fn byte_prefix_map_requires_advance_trie_to_surface_prefixes() {
    let map = BytePrefixMap::<usize>::new(CacheLogConfig::new(64, 64, 64));
    map.insert_dirty(b"ab:001".to_vec(), 1);
    map.insert_dirty(b"ab:002".to_vec(), 2);
    map.insert_dirty(b"zz:001".to_vec(), 3);

    let before = map.list_prefix(b"ab:", |_, value, _, _| *value, 10);
    assert!(before.is_empty());

    let applied = map.advance_trie(64);
    assert!(applied >= 2);

    let mut rows = map.list_prefix(b"ab:", |_, value, _, _| *value, 10);
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

    let mut rows = map.list_prefix(b"ab:", |_, value, _, _| *value, 10);
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
    assert_eq!(
        map.list_prefix(b"ab:", |_, value, _, _| *value, 10),
        vec![2]
    );
}

#[test]
fn insert_dirty_batch_strict_keeps_all_writes_in_order() {
    let map = CacheLogMap::<String, usize>::new(CacheLogConfig::new(32, 32, 32));

    let ids = map.insert_dirty_batch(vec![
        ("same".to_owned(), 1),
        ("same".to_owned(), 2),
        ("same".to_owned(), 3),
    ]);

    assert_eq!(ids.len(), 3);
    let batch = map.flush_batch(10);
    let values = batch.iter().map(|entry| entry.value).collect::<Vec<_>>();
    assert_eq!(values, vec![1, 2, 3]);
}

#[test]
fn insert_dirty_batch_coalesced_keeps_last_per_key_for_batch() {
    let cfg = CacheLogConfig::new(32, 32, 32).with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
    let map = CacheLogMap::<String, usize>::new(cfg);

    let ids = map.insert_dirty_batch(vec![
        ("same".to_owned(), 1),
        ("same".to_owned(), 2),
        ("other".to_owned(), 9),
        ("same".to_owned(), 3),
    ]);

    assert_eq!(ids.len(), 2);
    assert!(matches!(
        read_triplet(&map, &"same".to_owned()),
        Some((3, EntryState::Dirty, VisibleRef::Dirty(_)))
    ));
    assert!(matches!(
        read_triplet(&map, &"other".to_owned()),
        Some((9, EntryState::Dirty, VisibleRef::Dirty(_)))
    ));

    let batch = map.flush_batch(10);
    assert_eq!(batch.len(), 2);
    let mut rows = batch
        .iter()
        .map(|entry| (entry.key.clone(), entry.value))
        .collect::<Vec<_>>();
    rows.sort();
    assert_eq!(rows, vec![("other".to_owned(), 9), ("same".to_owned(), 3)]);
}

fn all_alloc_modes() -> [DirtyAllocMode; 3] {
    [
        DirtyAllocMode::OwnedPerWrite,
        DirtyAllocMode::PooledVec,
        DirtyAllocMode::ChunkedArena,
    ]
}

fn run_dirty_backend_order_case(backend: DirtyQueueBackend) {
    for alloc_mode in all_alloc_modes() {
        let cfg = CacheLogConfig::new(64, 64, 64)
            .with_dirty_queue_backend(backend)
            .with_dirty_alloc_mode(alloc_mode);
        let map = CacheLogMap::<String, usize>::new(cfg);

        let ids = map.insert_dirty_batch(vec![
            ("k1".to_owned(), 1),
            ("k2".to_owned(), 2),
            ("k3".to_owned(), 3),
        ]);
        assert_eq!(ids.len(), 3);

        let batch = map.flush_batch(10);
        assert_eq!(batch.len(), 3);
        let rows = batch
            .iter()
            .map(|entry| (entry.id, entry.key.clone(), entry.value))
            .collect::<Vec<_>>();

        assert_eq!(rows[0].1, "k1");
        assert_eq!(rows[1].1, "k2");
        assert_eq!(rows[2].1, "k3");
        assert_eq!(map.mark_flushed(&batch), 3);
        assert_eq!(map.dirty_log_len(), 0);
    }
}

#[test]
fn dirty_queue_backend_kanal_preserves_order_and_flush() {
    run_dirty_backend_order_case(DirtyQueueBackend::Kanal);
}

#[test]
fn dirty_queue_backend_crossbeam_preserves_order_and_flush() {
    run_dirty_backend_order_case(DirtyQueueBackend::Crossbeam);
}

#[test]
fn dirty_queue_backend_stdsync_preserves_order_and_flush() {
    run_dirty_backend_order_case(DirtyQueueBackend::StdSync);
}

#[test]
fn dirty_alloc_modes_preserve_strict_batch_order() {
    for alloc_mode in all_alloc_modes() {
        let cfg = CacheLogConfig::new(16, 16, 16)
            .with_dirty_write_mode(DirtyWriteMode::StrictLog)
            .with_dirty_alloc_mode(alloc_mode)
            .with_dirty_queue_backend(DirtyQueueBackend::Kanal);
        let map = CacheLogMap::<String, usize>::new(cfg);

        let _ = map.insert_dirty_batch(vec![
            ("k1".to_owned(), 1),
            ("k2".to_owned(), 2),
            ("k3".to_owned(), 3),
            ("k4".to_owned(), 4),
        ]);

        let batch = map.flush_batch(8);
        let values = batch.iter().map(|entry| entry.value).collect::<Vec<_>>();
        assert_eq!(values, vec![1, 2, 3, 4]);
        assert_eq!(map.mark_flushed(&batch), 4);
        assert_eq!(map.dirty_log_len(), 0);
    }
}

fn run_dirty_backend_concurrent_flush_case(backend: DirtyQueueBackend) {
    for alloc_mode in all_alloc_modes() {
        let cfg = CacheLogConfig::new(2048, 2048, 64)
            .with_dirty_queue_backend(backend)
            .with_dirty_alloc_mode(alloc_mode);
        let map = Arc::new(CacheLogMap::<usize, usize>::new(cfg));

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
                    let _ = map.insert_dirty(k, k);
                }
            }));
        }

        let map_f = map.clone();
        let done_f = done.clone();
        let flushed_f = flushed.clone();
        let flusher = thread::spawn(move || {
            loop {
                let batch = map_f.flush_batch(64);
                if batch.is_empty() {
                    if done_f.load(Ordering::Acquire) {
                        break;
                    }
                    thread::yield_now();
                    continue;
                }
                let n = map_f.mark_flushed(&batch);
                flushed_f.fetch_add(n, Ordering::AcqRel);
            }
        });

        for w in writers {
            w.join().expect("writer join");
        }
        done.store(true, Ordering::Release);
        flusher.join().expect("flusher join");

        // Drain any tail race after done signal.
        loop {
            let batch = map.flush_batch(64);
            if batch.is_empty() {
                break;
            }
            let n = map.mark_flushed(&batch);
            flushed.fetch_add(n, Ordering::AcqRel);
        }

        let flushed_count = flushed.load(Ordering::Acquire);
        let dirty_len = map.dirty_log_len();
        let visible_len = map.visible_len();
        let mut leftovers = Vec::new();
        for key in 0..total {
            if let Some((value, state, visible)) = map.read(&key, |_, v, s, vr| (*v, s, vr)) {
                leftovers.push((key, value, state, visible));
                if leftovers.len() >= 16 {
                    break;
                }
            }
        }
        assert_eq!(
            flushed_count, total,
            "backend={backend:?} alloc={alloc_mode:?} flushed={flushed_count} total={total}"
        );
        assert_eq!(
            dirty_len, 0,
            "backend={backend:?} alloc={alloc_mode:?} dirty_len={dirty_len}"
        );
        assert_eq!(
            visible_len, 0,
            "backend={backend:?} alloc={alloc_mode:?} visible_len={visible_len} leftovers={leftovers:?}"
        );
    }
}

#[test]
fn dirty_queue_backend_kanal_concurrent_flushes_all() {
    run_dirty_backend_concurrent_flush_case(DirtyQueueBackend::Kanal);
}

#[test]
fn dirty_queue_backend_crossbeam_concurrent_flushes_all() {
    run_dirty_backend_concurrent_flush_case(DirtyQueueBackend::Crossbeam);
}

#[test]
fn dirty_queue_backend_stdsync_concurrent_flushes_all() {
    run_dirty_backend_concurrent_flush_case(DirtyQueueBackend::StdSync);
}

#[test]
fn coalesced_dirty_log_len_tracks_visible_dirty_backlog() {
    let cfg = CacheLogConfig::new(32, 32, 32).with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
    let map = CacheLogMap::<String, usize>::new(cfg);

    map.insert_dirty("a".to_owned(), 1);
    map.insert_dirty("b".to_owned(), 2);
    map.insert_dirty("a".to_owned(), 3);

    assert_eq!(map.dirty_log_len(), 2);

    let batch = map.flush_batch(16);
    assert_eq!(map.mark_flushed(&batch), batch.len());
    assert_eq!(map.dirty_log_len(), 0);
    assert!(map.read(&"a".to_owned(), |_, v, s, r| (*v, s, r)).is_none());
    assert!(map.read(&"b".to_owned(), |_, v, s, r| (*v, s, r)).is_none());
}
