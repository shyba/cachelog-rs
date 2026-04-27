#![cfg(feature = "loom")]

mod loom_support;

use loom::sync::Arc;
use loom::thread;

use cachelog::{CacheLogConfig, CacheLogMap, EntryState};
use loom_support::{STACK, assert_public_consistency, assert_snapshot_legal, run_exhaustive_model};

const SMALL_CONFIGS: [CacheLogConfig; 4] = [
    CacheLogConfig::new(1, 1, 1),
    CacheLogConfig::new(2, 1, 1),
    CacheLogConfig::new(2, 2, 1),
    CacheLogConfig::new(2, 2, 2),
];

#[test]
#[ignore = "slow exhaustive loom check"]
fn exhaustive_newer_dirty_survives_flush_of_older() {
    for config in SMALL_CONFIGS {
        run_exhaustive_model(5, 100_000, 20_000, Some(2), move || {
            let map = Arc::new(CacheLogMap::<usize, usize>::new(config));

            let m = map.clone();
            let setup = thread::Builder::new()
                .stack_size(STACK)
                .spawn(move || {
                    m.low_level().insert_dirty(1, 10);
                    m.low_level().flush_batch(1)
                })
                .unwrap();
            let batch = setup.join().unwrap();

            let m = map.clone();
            let writer = thread::Builder::new()
                .stack_size(STACK)
                .spawn(move || {
                    m.low_level().insert_dirty(1, 20);
                })
                .unwrap();

            let m2 = map.clone();
            let b = batch.clone();
            let flusher = thread::Builder::new()
                .stack_size(STACK)
                .spawn(move || {
                    m2.low_level().mark_flushed(&b);
                })
                .unwrap();

            writer.join().unwrap();
            flusher.join().unwrap();

            let result = map.read(&1, |_, v, s| (*v, s));
            if let Some((val, state)) = result {
                assert_eq!(val, 20);
                assert_eq!(state, EntryState::Dirty);
            }
            assert_public_consistency(&map, 1);
            assert_snapshot_legal(&map.low_level().debug_snapshot());
            loop {
                let batch = map.low_level().flush_batch(8);
                if batch.is_empty() {
                    break;
                }
                let _ = map.low_level().mark_flushed(&batch);
            }
            assert_eq!(map.dirty_log_len(), 0);
        });
    }
}

#[test]
#[ignore = "slow exhaustive loom check"]
fn exhaustive_same_key_concurrent_writers_leave_legal_state() {
    let config = CacheLogConfig::new(2, 2, 2);
    run_exhaustive_model(5, 100_000, 20_000, Some(2), move || {
        let map = Arc::new(CacheLogMap::<usize, usize>::new(config));

        let m1 = map.clone();
        let writer1 = thread::Builder::new()
            .stack_size(STACK)
            .spawn(move || {
                m1.low_level().insert_dirty(1, 10);
            })
            .unwrap();

        let m2 = map.clone();
        let writer2 = thread::Builder::new()
            .stack_size(STACK)
            .spawn(move || {
                m2.low_level().insert_dirty(1, 20);
            })
            .unwrap();

        writer1.join().unwrap();
        writer2.join().unwrap();

        let result = map.read(&1, |_, v, s| (*v, s));
        assert!(result.is_some());
        let (val, state) = result.unwrap();
        assert!(matches!(val, 10 | 20));
        assert_eq!(state, EntryState::Dirty);
        assert_public_consistency(&map, 1);
        assert_snapshot_legal(&map.low_level().debug_snapshot());
        loop {
            let batch = map.low_level().flush_batch(8);
            if batch.is_empty() {
                break;
            }
            let _ = map.low_level().mark_flushed(&batch);
        }
        assert_eq!(map.dirty_log_len(), 0);
    });
}
