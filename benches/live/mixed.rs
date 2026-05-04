use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use cachelog::{CacheLogConfig, CacheLogMap};
use criterion::{BenchmarkId, Criterion, Throughput};

pub fn bench_mixed_rw(c: &mut Criterion) {
    let mut group = c.benchmark_group("live");
    group.measurement_time(Duration::from_secs(3));
    group.throughput(Throughput::Elements(1));
    group.bench_function(BenchmarkId::new("mixed_rw", "write_with_4_readers"), |b| {
        b.iter_custom(|iters| {
            let map = Arc::new(CacheLogMap::<u64, u64>::new(CacheLogConfig::new(
                1 << 20,
                1 << 20,
                1 << 20,
            )));
            for i in 0_u64..(1 << 16) {
                map.low_level().insert_dirty(i, i);
            }
            let stop = Arc::new(AtomicBool::new(false));
            let mut readers = Vec::with_capacity(4);
            for t in 0..4_u64 {
                let reader_map = Arc::clone(&map);
                let reader_stop = Arc::clone(&stop);
                readers.push(thread::spawn(move || {
                    let mut x = 0x9E37_79B9_7F4A_7C15_u64 ^ t;
                    while !reader_stop.load(Ordering::Acquire) {
                        x ^= x << 13;
                        x ^= x >> 7;
                        x ^= x << 17;
                        let key = x & ((1 << 16) - 1);
                        black_box(reader_map.read(&key, |_, value, _| *value));
                    }
                }));
            }
            let start = std::time::Instant::now();
            for i in 0..iters {
                let v = i;
                let key = v & ((1 << 16) - 1);
                black_box(map.low_level().insert_dirty(key, v));
            }
            let elapsed = start.elapsed();
            stop.store(true, Ordering::Release);
            for r in readers {
                r.join().unwrap();
            }
            elapsed
        });
    });
    group.finish();
}
