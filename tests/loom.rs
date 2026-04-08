#![cfg(feature = "loom")]

mod loom_support;

use loom::sync::Arc;
use loom::thread;

use cachelog::{CacheLogConfig, CacheLogMap, EntryState, VisibleRef};
use loom_support::{STACK, assert_public_consistency, assert_snapshot_legal, run_fast_model};

#[test]
fn newer_dirty_survives_flush_of_older() {
    run_fast_model(|| {
        let map = Arc::new(CacheLogMap::<usize, usize>::new(CacheLogConfig::new(
            4, 4, 4,
        )));

        let m = map.clone();
        let setup = thread::Builder::new()
            .stack_size(STACK)
            .spawn(move || {
                m.insert_dirty(1, 10);
                m.flush_batch(1)
            })
            .unwrap();
        let batch = setup.join().unwrap();

        let m = map.clone();
        let writer = thread::Builder::new()
            .stack_size(STACK)
            .spawn(move || {
                m.insert_dirty(1, 20);
            })
            .unwrap();

        let m2 = map.clone();
        let b = batch.clone();
        let flusher = thread::Builder::new()
            .stack_size(STACK)
            .spawn(move || {
                m2.mark_flushed(&b);
            })
            .unwrap();

        writer.join().unwrap();
        flusher.join().unwrap();

        let m = map.clone();
        let checker = thread::Builder::new()
            .stack_size(STACK)
            .spawn(move || {
                if let Some((val, state, _)) = m.read(&1, |_, v, s, r| (*v, s, r)) {
                    assert_eq!(val, 20);
                    assert_eq!(state, EntryState::Dirty);
                }
            })
            .unwrap();
        checker.join().unwrap();
        assert_snapshot_legal(&map.debug_snapshot());
    });
}

#[test]
fn concurrent_read_and_write() {
    run_fast_model(|| {
        let map = Arc::new(CacheLogMap::<usize, usize>::new(CacheLogConfig::new(
            4, 4, 4,
        )));

        let m = map.clone();
        let writer = thread::Builder::new()
            .stack_size(STACK)
            .spawn(move || {
                m.insert_dirty(1, 42);
            })
            .unwrap();

        let m2 = map.clone();
        let reader = thread::Builder::new()
            .stack_size(STACK)
            .spawn(move || m2.read(&1, |_, v, s, _| (*v, s)))
            .unwrap();

        writer.join().unwrap();
        let result = reader.join().unwrap();

        match result {
            None => {}
            Some((val, state)) => {
                assert_eq!(val, 42);
                assert_eq!(state, EntryState::Dirty);
            }
        }
        assert_snapshot_legal(&map.debug_snapshot());
    });
}

#[test]
fn dirty_write_replaces_clean() {
    run_fast_model(|| {
        let map = Arc::new(CacheLogMap::<usize, usize>::new(CacheLogConfig::new(
            4, 4, 4,
        )));

        let m = map.clone();
        let setup = thread::Builder::new()
            .stack_size(STACK)
            .spawn(move || {
                m.insert_clean_if_absent(1, 10);
            })
            .unwrap();
        setup.join().unwrap();

        let m = map.clone();
        let writer = thread::Builder::new()
            .stack_size(STACK)
            .spawn(move || {
                m.insert_dirty(1, 20);
            })
            .unwrap();

        writer.join().unwrap();

        let m = map.clone();
        let checker = thread::Builder::new()
            .stack_size(STACK)
            .spawn(move || {
                let result = m.read(&1, |_, v, s, _| (*v, s));
                assert_eq!(result, Some((20, EntryState::Dirty)));
            })
            .unwrap();
        checker.join().unwrap();
        assert_snapshot_legal(&map.debug_snapshot());
    });
}

#[test]
fn flush_does_not_clear_newer_dirty_ptr() {
    run_fast_model(|| {
        let map = Arc::new(CacheLogMap::<usize, usize>::new(CacheLogConfig::new(
            4, 4, 4,
        )));

        let m = map.clone();
        let setup = thread::Builder::new()
            .stack_size(STACK)
            .spawn(move || {
                m.insert_dirty(1, 10);
                let batch = m.flush_batch(1);
                m.insert_dirty(1, 20);
                batch
            })
            .unwrap();
        let batch = setup.join().unwrap();

        let m = map.clone();
        let b = batch.clone();
        let flusher = thread::Builder::new()
            .stack_size(STACK)
            .spawn(move || {
                m.mark_flushed(&b);
            })
            .unwrap();

        flusher.join().unwrap();

        let m = map.clone();
        let checker = thread::Builder::new()
            .stack_size(STACK)
            .spawn(move || {
                let result = m.read(&1, |_, v, s, r| (*v, s, r));
                assert!(result.is_some());
                let (val, state, vis) = result.unwrap();
                assert_eq!(val, 20);
                assert_eq!(state, EntryState::Dirty);
                assert!(matches!(vis, VisibleRef::Dirty(_)));
            })
            .unwrap();
        checker.join().unwrap();
        assert_snapshot_legal(&map.debug_snapshot());
    });
}

#[test]
fn concurrent_clean_eviction() {
    run_fast_model(|| {
        let map = Arc::new(CacheLogMap::<usize, usize>::new(CacheLogConfig::new(
            4, 4, 4,
        )));

        let m = map.clone();
        let setup = thread::Builder::new()
            .stack_size(STACK)
            .spawn(move || {
                m.insert_clean_if_absent(1, 10);
            })
            .unwrap();
        setup.join().unwrap();

        let m = map.clone();
        let writer = thread::Builder::new()
            .stack_size(STACK)
            .spawn(move || {
                m.insert_dirty(1, 99);
            })
            .unwrap();

        let m2 = map.clone();
        let evictor = thread::Builder::new()
            .stack_size(STACK)
            .spawn(move || {
                m2.evict_clean(&1);
            })
            .unwrap();

        writer.join().unwrap();
        evictor.join().unwrap();

        let m = map.clone();
        let checker = thread::Builder::new()
            .stack_size(STACK)
            .spawn(move || {
                if let Some((val, state)) = m.read(&1, |_, v, s, _| (*v, s)) {
                    assert_eq!(val, 99);
                    assert_eq!(state, EntryState::Dirty);
                }
            })
            .unwrap();
        checker.join().unwrap();
        assert_snapshot_legal(&map.debug_snapshot());
    });
}

#[test]
fn concurrent_writers_same_key_leave_a_valid_dirty_value() {
    run_fast_model(|| {
        let map = Arc::new(CacheLogMap::<usize, usize>::new(CacheLogConfig::new(
            4, 4, 4,
        )));

        let m1 = map.clone();
        let writer1 = thread::Builder::new()
            .stack_size(STACK)
            .spawn(move || {
                m1.insert_dirty(1, 10);
            })
            .unwrap();

        let m2 = map.clone();
        let writer2 = thread::Builder::new()
            .stack_size(STACK)
            .spawn(move || {
                m2.insert_dirty(1, 20);
            })
            .unwrap();

        writer1.join().unwrap();
        writer2.join().unwrap();

        let result = map.read(&1, |_, v, s, r| (*v, s, r));
        assert!(result.is_some());
        let (val, state, vis) = result.unwrap();
        assert!(matches!(val, 10 | 20));
        assert_eq!(state, EntryState::Dirty);
        assert!(matches!(vis, VisibleRef::Dirty(_)));
        assert_snapshot_legal(&map.debug_snapshot());
    });
}

#[test]
fn public_api_is_consistent_after_write_race_joins() {
    run_fast_model(|| {
        let map = Arc::new(CacheLogMap::<usize, usize>::new(CacheLogConfig::new(
            4, 4, 4,
        )));

        let m1 = map.clone();
        let writer = thread::Builder::new()
            .stack_size(STACK)
            .spawn(move || {
                m1.insert_dirty(7, 70);
            })
            .unwrap();

        let m2 = map.clone();
        let reader = thread::Builder::new()
            .stack_size(STACK)
            .spawn(move || {
                let _ = m2.read(&7, |_, value, state, visible| (*value, state, visible));
            })
            .unwrap();

        writer.join().unwrap();
        reader.join().unwrap();

        assert_public_consistency(&map, 7);
        assert_snapshot_legal(&map.debug_snapshot());
    });
}
