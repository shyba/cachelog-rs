use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use cachelog::{CacheLogConfig, CacheLogMap};
use criterion::{BenchmarkId, Criterion, Throughput};

pub fn bench_dirty_read(c: &mut Criterion) {
    let mut group = c.benchmark_group("live");
    group.measurement_time(Duration::from_secs(3));
    group.throughput(Throughput::Elements(1));
    let dirty_reads = CacheLogMap::<u64, u64>::new(CacheLogConfig::new(1024, 1024, 1024));
    dirty_reads.low_level().insert_dirty(7, 77);
    group.bench_function(BenchmarkId::new("dirty_read", "serial_hit"), |b| {
        b.iter(|| black_box(dirty_reads.read(&7, |_, value, _| *value).unwrap()));
    });
    group.finish();
}

pub fn bench_borrowed_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("live");
    group.measurement_time(Duration::from_secs(3));
    group.throughput(Throughput::Elements(1));
    let string_reads = CacheLogMap::<String, u64>::new(CacheLogConfig::new(1024, 1024, 1024));
    string_reads
        .low_level()
        .insert_dirty("alpha".to_owned(), 77);
    let owned_string = "alpha".to_owned();
    group.bench_function(BenchmarkId::new("dirty_read_string", "owned_lookup"), |b| {
        b.iter(|| {
            black_box(
                string_reads
                    .read(&owned_string, |_, value, _| *value)
                    .unwrap(),
            )
        });
    });
    group.bench_function(
        BenchmarkId::new("dirty_read_string", "borrowed_lookup"),
        |b| {
            b.iter(|| black_box(string_reads.read("alpha", |_, value, _| *value).unwrap()));
        },
    );
    let byte_reads = CacheLogMap::<Vec<u8>, u64>::new(CacheLogConfig::new(1024, 1024, 1024));
    byte_reads.low_level().insert_dirty(b"alpha".to_vec(), 88);
    group.bench_function(
        BenchmarkId::new("dirty_read_bytes", "owned_lookup_with_clone"),
        |b| {
            b.iter(|| {
                let key = b"alpha".to_vec();
                black_box(byte_reads.read(&key, |_, value, _| *value).unwrap());
            });
        },
    );
    group.bench_function(
        BenchmarkId::new("dirty_read_bytes", "borrowed_lookup"),
        |b| {
            b.iter(|| {
                black_box(
                    byte_reads
                        .read(b"alpha".as_slice(), |_, value, _| *value)
                        .unwrap(),
                )
            });
        },
    );
    group.finish();
}

pub fn bench_dirty_read_under_write(c: &mut Criterion) {
    let mut group = c.benchmark_group("live");
    group.measurement_time(Duration::from_secs(3));
    group.throughput(Throughput::Elements(1));
    let map = Arc::new(CacheLogMap::<u64, u64>::new(CacheLogConfig::new(
        1 << 20,
        1 << 20,
        1 << 20,
    )));
    map.low_level().insert_dirty(7, 77);
    group.bench_function(
        BenchmarkId::new("dirty_read", "under_concurrent_write"),
        |b| {
            let stop = Arc::new(AtomicBool::new(false));
            let writer_map = Arc::clone(&map);
            let writer_stop = Arc::clone(&stop);
            let writer = thread::spawn(move || {
                let mut i = 0_u64;
                while !writer_stop.load(Ordering::Acquire) {
                    let key = i & ((1 << 16) - 1);
                    black_box(writer_map.low_level().insert_dirty(key, i));
                    i = i.wrapping_add(1);
                }
            });
            b.iter(|| black_box(map.read(&7, |_, value, _| *value).unwrap()));
            stop.store(true, Ordering::Release);
            writer.join().unwrap();
        },
    );
    group.finish();
}
