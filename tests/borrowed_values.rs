#![cfg(not(feature = "loom"))]

mod common;

use std::collections::BTreeSet;
use std::sync::{Arc, Barrier};
use std::thread;

use cachelog::{CacheLogConfig, CacheLogMap};

#[test]
fn borrowed_batch_insert_reserves_unique_ids_under_concurrency() {
    let map = Arc::new(CacheLogMap::<Vec<u8>, Vec<u8>>::new(CacheLogConfig::new(
        16_384, 16_384, 16,
    )));
    let start = Arc::new(Barrier::new(3));

    let map_batch = Arc::clone(&map);
    let start_batch = Arc::clone(&start);
    let batch_writer = thread::spawn(move || {
        start_batch.wait();
        for i in 0..512_u32 {
            let key1 = format!("batch-a-{i}");
            let val1 = format!("value-a-{i}");
            let key2 = format!("batch-b-{i}");
            let val2 = format!("value-b-{i}");
            map_batch
                .low_level()
                .insert_dirty_batch_borrowed_without_ids(
                    [
                        (key1.as_bytes(), val1.as_bytes()),
                        (key2.as_bytes(), val2.as_bytes()),
                    ]
                    .into_iter(),
                );
        }
    });

    let map_single = Arc::clone(&map);
    let start_single = Arc::clone(&start);
    let single_writer = thread::spawn(move || {
        start_single.wait();
        for i in 0..1024_u32 {
            map_single.low_level().insert_dirty(
                format!("single-{i}").into_bytes(),
                format!("single-value-{i}").into_bytes(),
            );
        }
    });

    start.wait();
    batch_writer.join().expect("batch writer join");
    single_writer.join().expect("single writer join");

    let mut ids = Vec::new();
    loop {
        let batch = map.low_level().flush_batch(4096);
        if batch.is_empty() {
            break;
        }
        ids.extend(batch.iter().map(|entry| entry.id));
        assert_eq!(map.low_level().mark_flushed(&batch), batch.len());
    }

    let expected = 2048_usize;
    assert_eq!(ids.len(), expected, "flushed ids len mismatch");
    let unique = ids.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(unique.len(), expected, "duplicate ids observed");
    assert_eq!(unique.first().copied(), Some(0));
    assert_eq!(unique.last().copied(), Some(expected as u64 - 1));
}
