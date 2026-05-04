#![cfg(not(feature = "loom"))]

mod common;

use std::sync::{Arc, Mutex};
use std::thread;

use cachelog::{BackgroundFlushConfig, CacheLogConfig, CacheLogMap, DirtyWriteMode};

#[test]
fn background_flush_config_for_batch_size_matches_integration_policy() {
    let small = BackgroundFlushConfig::for_batch_size(64);
    assert_eq!(small.trigger_dirty, 64);
    assert_eq!(small.flush_limit, 512);
    assert_eq!(small.check_interval, std::time::Duration::from_millis(100));

    let large = BackgroundFlushConfig::for_batch_size(256);
    assert_eq!(large.trigger_dirty, 256);
    assert_eq!(large.flush_limit, 512);
    assert_eq!(large.check_interval, std::time::Duration::from_secs(10));
}

#[test]
fn background_flush_default_entry_point_flushes_pending_writes() {
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

#[test]
fn background_flush_batch_size_entry_point_flushes_pending_writes() {
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

#[test]
fn background_flush_persist_error_stops_service() {
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

#[test]
fn background_flush_respects_flush_limit_per_persist_batch() {
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

#[test]
fn coalesced_background_flush_drains_backlog_below_trigger() {
    let cfg =
        CacheLogConfig::new(2048, 2048, 2048).with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
    let map = Arc::new(CacheLogMap::<u64, u64>::new(cfg));
    let persisted = Arc::new(Mutex::new(Vec::new()));
    let persisted_flush = Arc::clone(&persisted);
    let service = map.start_background_flush(
        BackgroundFlushConfig::new(1024, 512, std::time::Duration::from_millis(100)),
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

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while map.dirty_log_len() != 0 && std::time::Instant::now() < deadline {
        thread::sleep(std::time::Duration::from_millis(5));
    }

    let lens = persisted.lock().expect("persisted lock").clone();
    assert_eq!(lens.iter().sum::<usize>(), 1024, "persisted lens: {lens:?}");
    assert_eq!(map.dirty_log_len(), 0);
}

#[test]
fn background_flush_request_flush_drains_without_note_writes() {
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

#[test]
fn background_flush_check_interval_flushes_without_note_writes() {
    let map = Arc::new(CacheLogMap::<u64, u64>::new(CacheLogConfig::new(
        64, 64, 64,
    )));
    let persisted = Arc::new(Mutex::new(Vec::new()));
    let persisted_flush = Arc::clone(&persisted);
    let service = map.start_background_flush(
        BackgroundFlushConfig::new(1, 4, std::time::Duration::from_millis(10)),
        move |batch| {
            persisted_flush
                .lock()
                .expect("persisted lock")
                .push(batch.len());
            Ok(())
        },
    );

    map.low_level().insert_dirty(1, 11);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while map.dirty_log_len() != 0 && std::time::Instant::now() < deadline {
        thread::sleep(std::time::Duration::from_millis(5));
    }

    assert_eq!(map.dirty_log_len(), 0, "background flusher never polled");
    let lens = persisted.lock().expect("persisted lock").clone();
    assert_eq!(lens.iter().sum::<usize>(), 1, "persisted lens: {lens:?}");
    service.shutdown().expect("shutdown");
}

#[test]
fn background_flush_reconfigure_lower_threshold_wakes_existing_backlog() {
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

#[test]
fn background_flush_reconfigure_for_batch_size_wakes_existing_backlog() {
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

#[test]
fn background_flush_shutdown_unblocks_cloned_handle_flush_sync() {
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
