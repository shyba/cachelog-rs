use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;

use cachelog_rs::{CacheLogConfig, CacheLogMap, EntryState, VisibleRef};

fn read_triplet<K, V>(map: &CacheLogMap<K, V>, key: &K) -> Option<(V, EntryState, VisibleRef)>
where
    K: Clone + Eq + std::hash::Hash,
    V: Copy,
{
    map.read(key, |_, value, state, visible| (*value, state, visible))
}

#[test]
fn dirty_write_is_immediately_visible() {
    let map = CacheLogMap::<String, usize>::new(CacheLogConfig::new(16, 16, 16));

    let id = map.insert_dirty("alpha".to_owned(), 1);

    assert_eq!(id, 0);
    assert_eq!(
        read_triplet(&map, &"alpha".to_owned()),
        Some((1, EntryState::Dirty, VisibleRef::Dirty(0)))
    );
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

    assert_eq!(dirty_id, 0);
    assert_eq!(
        read_triplet(&map, &"gamma".to_owned()),
        Some((4, EntryState::Dirty, VisibleRef::Dirty(0)))
    );
}

#[test]
fn flush_batch_is_in_write_order() {
    let map = CacheLogMap::<String, usize>::new(CacheLogConfig::new(16, 16, 16));

    map.insert_dirty("a".to_owned(), 1);
    map.insert_dirty("b".to_owned(), 2);
    map.insert_dirty("c".to_owned(), 3);

    let batch = map.flush_batch(2);
    let ids = batch.iter().map(|entry| entry.id).collect::<Vec<_>>();
    let keys = batch
        .iter()
        .map(|entry| entry.key.clone())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec![0, 1]);
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

    let old_id = map.insert_dirty("same".to_owned(), 1);
    let new_id = map.insert_dirty("same".to_owned(), 2);

    let batch = map.flush_batch(1);
    assert_eq!(
        batch.iter().map(|entry| entry.id).collect::<Vec<_>>(),
        vec![old_id]
    );
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
    assert_eq!(
        first.iter().map(|entry| entry.id).collect::<Vec<_>>(),
        retry.iter().map(|entry| entry.id).collect::<Vec<_>>()
    );
    assert_eq!(map.dirty_log_len(), 2);

    assert_eq!(map.mark_flushed(&first), 1);
    let next = map.flush_batch(1);
    assert_eq!(
        next.iter().map(|entry| entry.id).collect::<Vec<_>>(),
        vec![1]
    );
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
        assert_eq!(
            read_triplet(&map, &key),
            Some((key * 10, EntryState::Dirty, VisibleRef::Dirty(key as u64)))
        );
    }
}
