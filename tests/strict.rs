#![cfg(not(feature = "loom"))]

mod common;

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::thread;

use bytes::Bytes;
use cachelog::{CacheLogConfig, CacheLogMap, EntryState};
use common::read_triplet;

#[test]
fn borrowed_string_lookup_works_across_low_level_api() {
    let map = CacheLogMap::<String, usize>::new(CacheLogConfig::new(16, 16, 16));
    let id = map.low_level().insert_dirty("borrowed".to_owned(), 9);

    assert_eq!(
        map.low_level().visible_ref("borrowed"),
        Some(cachelog::low_level::VisibleRef::Dirty(id))
    );
    assert_eq!(
        map.low_level().get_cloned_full("borrowed"),
        Some((
            9,
            EntryState::Dirty,
            cachelog::low_level::VisibleRef::Dirty(id)
        ))
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
            cachelog::low_level::VisibleRef::Dirty(id)
        ))
    );
}

#[test]
fn borrowed_bytes_lookup_works_across_low_level_api() {
    let map = CacheLogMap::<Vec<u8>, usize>::new(CacheLogConfig::new(16, 16, 16));
    let id = map.low_level().insert_dirty(b"bytes".to_vec(), 11);

    assert_eq!(
        map.low_level().visible_ref(b"bytes".as_slice()),
        Some(cachelog::low_level::VisibleRef::Dirty(id))
    );
    assert_eq!(
        map.low_level().get_cloned_full(b"bytes".as_slice()),
        Some((
            11,
            EntryState::Dirty,
            cachelog::low_level::VisibleRef::Dirty(id)
        ))
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
            cachelog::low_level::VisibleRef::Dirty(id)
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
            cachelog::low_level::VisibleRef::Dirty(id1)
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
            cachelog::low_level::VisibleRef::Dirty(id2)
        ))
    );

    let inserted = map.low_level().insert_dirty_batch_without_ids(vec![
        (b"bytes:b1".to_vec(), Bytes::from_static(b"v1")),
        (b"bytes:b2".to_vec(), Bytes::from_static(b"v2")),
    ]);
    assert_eq!(inserted, 2);
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
        Some((
            2,
            EntryState::Dirty,
            cachelog::low_level::VisibleRef::Dirty(new_id)
        ))
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
        Some((
            2,
            EntryState::Dirty,
            cachelog::low_level::VisibleRef::Dirty(newer)
        ))
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
    let cfg =
        CacheLogConfig::new(16, 16, 16).with_dirty_write_mode(cachelog::DirtyWriteMode::StrictLog);
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
