#![cfg(feature = "loom")]

use loom::sync::Arc;
use loom::thread;

use cachelog_rs::{CacheLogConfig, CacheLogMap, EntryState, VisibleRef};

const STACK: usize = 4 * 1024 * 1024;

#[test]
fn newer_dirty_survives_flush_of_older() {
    loom::model(|| {
        let map = Arc::new(CacheLogMap::<usize, usize>::new(CacheLogConfig::new(4, 4, 4)));

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
    });
}

#[test]
fn concurrent_read_and_write() {
    loom::model(|| {
        let map = Arc::new(CacheLogMap::<usize, usize>::new(CacheLogConfig::new(4, 4, 4)));

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
    });
}

#[test]
fn dirty_write_replaces_clean() {
    loom::model(|| {
        let map = Arc::new(CacheLogMap::<usize, usize>::new(CacheLogConfig::new(4, 4, 4)));

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
    });
}

#[test]
fn flush_does_not_clear_newer_dirty_ptr() {
    loom::model(|| {
        let map = Arc::new(CacheLogMap::<usize, usize>::new(CacheLogConfig::new(4, 4, 4)));

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
    });
}

#[test]
fn concurrent_clean_eviction() {
    loom::model(|| {
        let map = Arc::new(CacheLogMap::<usize, usize>::new(CacheLogConfig::new(4, 4, 4)));

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
    });
}
