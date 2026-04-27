#![cfg(not(feature = "loom"))]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use cachelog::{CacheLogConfig, CacheLogMap, DirtyWriteMode, EntryState};

fn map_for_mode(mode: DirtyWriteMode) -> CacheLogMap<usize, usize> {
    CacheLogMap::new(CacheLogConfig::new(256, 256, 64).with_dirty_write_mode(mode))
}

#[test]
fn common_concurrent_reads_stay_last_write_wins_in_both_modes() {
    for mode in [DirtyWriteMode::StrictLog, DirtyWriteMode::CoalescedMap] {
        let map = Arc::new(map_for_mode(mode));
        let started = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let published = Arc::new(AtomicUsize::new(0));

        let writer_map = Arc::clone(&map);
        let writer_started = Arc::clone(&started);
        let writer_done = Arc::clone(&done);
        let writer_published = Arc::clone(&published);
        let writer = thread::spawn(move || {
            while !writer_started.load(Ordering::Acquire) {
                thread::yield_now();
            }
            for key in 0..256usize {
                writer_map.put(key, key * 10);
                writer_published.store(key + 1, Ordering::Release);
            }
            writer_done.store(true, Ordering::Release);
        });

        let mut readers = Vec::new();
        for _ in 0..3 {
            let reader_map = Arc::clone(&map);
            let reader_started = Arc::clone(&started);
            let reader_done = Arc::clone(&done);
            let reader_published = Arc::clone(&published);
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
        writer.join().expect("writer join");
        for reader in readers {
            reader.join().expect("reader join");
        }

        for key in 0..256usize {
            assert_eq!(
                map.read(&key, |_, value, state| (*value, state)),
                Some((key * 10, EntryState::Dirty)),
                "mode={mode:?} key={key}"
            );
        }
    }
}

#[test]
fn common_with_persisted_scan_keeps_newer_concurrent_writes_visible_in_both_modes() {
    for mode in [DirtyWriteMode::StrictLog, DirtyWriteMode::CoalescedMap] {
        let map = Arc::new(map_for_mode(mode));
        map.put(1, 10);
        map.put(2, 20);

        let persist_started = Arc::new(AtomicBool::new(false));
        let persisted = Arc::new(Mutex::new(Vec::new()));

        let writer_map = Arc::clone(&map);
        let writer_started = Arc::clone(&persist_started);
        let writer = thread::spawn(move || {
            while !writer_started.load(Ordering::Acquire) {
                thread::yield_now();
            }
            writer_map.put(1, 100);
            writer_map.put(3, 300);
        });

        let scan_result = map
            .with_persisted_scan(
                16,
                |batch| {
                    persisted
                        .lock()
                        .expect("persisted lock")
                        .extend(batch.iter().map(|entry| (*entry.key(), *entry.value())));
                    persist_started.store(true, Ordering::Release);
                    thread::sleep(std::time::Duration::from_millis(10));
                    Ok::<_, &'static str>(())
                },
                || Ok::<_, &'static str>("scan-ok"),
            )
            .expect("with_persisted_scan");
        writer.join().expect("writer join");
        assert_eq!(scan_result, "scan-ok");

        let mut rows = persisted.lock().expect("persisted lock").clone();
        rows.sort_unstable();
        assert!(
            rows.contains(&(1, 10)) && rows.contains(&(2, 20)),
            "mode={mode:?} persisted rows: {rows:?}"
        );

        let key1 = map.read(&1, |_, value, state| (*value, state));
        assert!(
            matches!(key1, None | Some((100, EntryState::Dirty))),
            "mode={mode:?} key1={key1:?}"
        );
        let key3 = map.read(&3, |_, value, state| (*value, state));
        assert!(
            matches!(key3, None | Some((300, EntryState::Dirty))),
            "mode={mode:?} key3={key3:?}"
        );
        assert!(map.read(&2, |_, value, state| (*value, state)).is_none());
    }
}

#[test]
fn common_flush_now_drains_unique_key_backlog_in_both_modes() {
    for mode in [DirtyWriteMode::StrictLog, DirtyWriteMode::CoalescedMap] {
        let map = map_for_mode(mode);
        let batch_rows: Vec<(usize, usize)> = (0..64usize).map(|key| (key, key * 10)).collect();
        let expected: BTreeMap<usize, usize> = batch_rows.iter().copied().collect();

        assert_eq!(map.put_batch(batch_rows), expected.len());

        let persisted = Arc::new(Mutex::new(Vec::new()));
        let persisted_flush = Arc::clone(&persisted);
        let flushed = map
            .flush_now(16, move |batch| {
                persisted_flush
                    .lock()
                    .expect("persisted lock")
                    .extend(batch.iter().map(|entry| (*entry.key(), *entry.value())));
                Ok::<_, &'static str>(())
            })
            .expect("flush_now");

        assert_eq!(flushed, expected.len(), "mode={mode:?}");

        let mut rows = persisted.lock().expect("persisted lock").clone();
        rows.sort_unstable();
        assert_eq!(
            rows,
            expected.into_iter().collect::<Vec<_>>(),
            "mode={mode:?}"
        );

        for key in 0..64usize {
            assert!(
                map.read(&key, |_, value, state| (*value, state)).is_none(),
                "mode={mode:?} key={key}"
            );
        }
    }
}
