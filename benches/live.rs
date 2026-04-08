use std::hint::black_box;
use std::time::Duration;

use cachelog_rs::{CacheLogConfig, CacheLogMap};
use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

fn bench_dirty_write(c: &mut Criterion) {
    let mut group = c.benchmark_group("live");
    group.measurement_time(Duration::from_secs(3));
    group.throughput(Throughput::Elements(1));

    group.bench_function(BenchmarkId::new("dirty_write", "serial_insert"), |b| {
        let writes = CacheLogMap::<u64, u64>::new(CacheLogConfig::new(1 << 20, 1 << 20, 16));
        let mut key = 0_u64;
        b.iter(|| {
            let id = writes.insert_dirty(key, key);
            black_box(id);
            key = key.wrapping_add(1);
        });
    });

    group.finish();
}

fn bench_dirty_read(c: &mut Criterion) {
    let mut group = c.benchmark_group("live");
    group.measurement_time(Duration::from_secs(3));
    group.throughput(Throughput::Elements(1));

    let dirty_reads = CacheLogMap::<u64, u64>::new(CacheLogConfig::new(1024, 1024, 1024));
    dirty_reads.insert_dirty(7, 77);

    group.bench_function(BenchmarkId::new("dirty_read", "serial_hit"), |b| {
        b.iter(|| {
            let result = dirty_reads.read(&7, |_, value, _, _| *value).unwrap();
            black_box(result);
        });
    });

    group.finish();
}

fn bench_borrowed_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("live");
    group.measurement_time(Duration::from_secs(3));
    group.throughput(Throughput::Elements(1));

    let string_reads = CacheLogMap::<String, u64>::new(CacheLogConfig::new(1024, 1024, 1024));
    string_reads.insert_dirty("alpha".to_owned(), 77);
    let owned_string = "alpha".to_owned();

    group.bench_function(BenchmarkId::new("dirty_read_string", "owned_lookup"), |b| {
        b.iter(|| {
            let result = string_reads
                .read(&owned_string, |_, value, _, _| *value)
                .unwrap();
            black_box(result);
        });
    });

    group.bench_function(
        BenchmarkId::new("dirty_read_string", "borrowed_lookup"),
        |b| {
            b.iter(|| {
                let result = string_reads.read("alpha", |_, value, _, _| *value).unwrap();
                black_box(result);
            });
        },
    );

    let byte_reads = CacheLogMap::<Vec<u8>, u64>::new(CacheLogConfig::new(1024, 1024, 1024));
    byte_reads.insert_dirty(b"alpha".to_vec(), 88);

    group.bench_function(
        BenchmarkId::new("dirty_read_bytes", "owned_lookup_with_clone"),
        |b| {
            b.iter(|| {
                let key = b"alpha".to_vec();
                let result = byte_reads.read(&key, |_, value, _, _| *value).unwrap();
                black_box(result);
            });
        },
    );

    group.bench_function(
        BenchmarkId::new("dirty_read_bytes", "borrowed_lookup"),
        |b| {
            b.iter(|| {
                let result = byte_reads
                    .read(b"alpha".as_slice(), |_, value, _, _| *value)
                    .unwrap();
                black_box(result);
            });
        },
    );

    group.finish();
}

fn bench_dirty_batch_write(c: &mut Criterion) {
    let mut group = c.benchmark_group("live");
    group.measurement_time(Duration::from_secs(3));

    for batch_size in [1_usize, 16, 64, 256] {
        group.throughput(Throughput::Elements(batch_size as u64));

        group.bench_with_input(
            BenchmarkId::new("dirty_batch_write", format!("per_entry_{batch_size}")),
            &batch_size,
            |b, &batch_size| {
                let writes =
                    CacheLogMap::<u64, u64>::new(CacheLogConfig::new(1 << 20, 1 << 20, 16));
                let mut base = 0_u64;
                b.iter_batched(
                    || {
                        let start = base;
                        base = base.wrapping_add(batch_size as u64);
                        start
                    },
                    |start| {
                        for offset in 0..batch_size {
                            black_box(writes.insert_dirty(
                                start.wrapping_add(offset as u64),
                                start.wrapping_add(offset as u64),
                            ));
                        }
                    },
                    BatchSize::SmallInput,
                );
            },
        );

        group.bench_with_input(
            BenchmarkId::new("dirty_batch_write", format!("bulk_{batch_size}")),
            &batch_size,
            |b, &batch_size| {
                let writes =
                    CacheLogMap::<u64, u64>::new(CacheLogConfig::new(1 << 20, 1 << 20, 16));
                let mut base = 0_u64;
                b.iter_batched(
                    || {
                        let start = base;
                        base = base.wrapping_add(batch_size as u64);
                        (0..batch_size)
                            .map(|offset| {
                                let key = start.wrapping_add(offset as u64);
                                (key, key)
                            })
                            .collect::<Vec<_>>()
                    },
                    |entries| {
                        black_box(writes.insert_dirty_batch(entries));
                    },
                    BatchSize::SmallInput,
                );
            },
        );

        group.bench_with_input(
            BenchmarkId::new("dirty_batch_write", format!("bulk_no_ids_{batch_size}")),
            &batch_size,
            |b, &batch_size| {
                let writes =
                    CacheLogMap::<u64, u64>::new(CacheLogConfig::new(1 << 20, 1 << 20, 16));
                let mut base = 0_u64;
                b.iter_batched(
                    || {
                        let start = base;
                        base = base.wrapping_add(batch_size as u64);
                        (0..batch_size)
                            .map(|offset| {
                                let key = start.wrapping_add(offset as u64);
                                (key, key)
                            })
                            .collect::<Vec<_>>()
                    },
                    |entries| {
                        writes.insert_dirty_batch_without_ids(entries);
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

fn bench_stale_cycle(c: &mut Criterion) {
    let mut group = c.benchmark_group("live");
    group.measurement_time(Duration::from_secs(3));
    group.throughput(Throughput::Elements(1));

    let stale = CacheLogMap::<u64, u64>::new(CacheLogConfig::new(1024, 1024, 1024));
    let _ = stale.insert_clean_if_absent(1, 1);

    group.bench_function(
        BenchmarkId::new("stale_cycle", "read_cleanup_reinsert"),
        |b| {
            b.iter(|| {
                black_box(stale.evict_clean(&1));
                black_box(stale.read(&1, |_, value, _, _| *value));
                black_box(stale.insert_clean_if_absent(1, 1));
            });
        },
    );

    group.finish();
}

fn bench_stale_breakdown(c: &mut Criterion) {
    let mut group = c.benchmark_group("live");
    group.measurement_time(Duration::from_secs(3));
    group.throughput(Throughput::Elements(1));

    let evict_only = CacheLogMap::<u64, u64>::new(CacheLogConfig::new(1024, 1024, 1024));
    let _ = evict_only.insert_clean_if_absent(1, 1);
    group.bench_function(BenchmarkId::new("stale_breakdown", "evict_clean"), |b| {
        b.iter(|| {
            black_box(evict_only.evict_clean(&1));
            black_box(evict_only.insert_clean_if_absent(1, 1));
        });
    });

    let miss_only = CacheLogMap::<u64, u64>::new(CacheLogConfig::new(1024, 1024, 1024));
    group.bench_function(BenchmarkId::new("stale_breakdown", "read_miss"), |b| {
        b.iter(|| {
            black_box(miss_only.read(&1, |_, value, _, _| *value));
        });
    });

    group.bench_function(
        BenchmarkId::new("stale_breakdown", "insert_clean_if_absent"),
        |b| {
            b.iter_custom(|iters| {
                let insert_only = CacheLogMap::<u64, u64>::new(CacheLogConfig::new(
                    iters as usize + 16,
                    16,
                    iters as usize + 16,
                ));
                let mut key = 0_u64;
                let start = std::time::Instant::now();
                for _ in 0..iters {
                    black_box(insert_only.insert_clean_if_absent(key, key));
                    key = key.wrapping_add(1);
                }
                start.elapsed()
            });
        },
    );

    group.finish();
}

criterion_group!(
    benches,
    bench_dirty_write,
    bench_dirty_read,
    bench_borrowed_lookup,
    bench_dirty_batch_write,
    bench_stale_cycle,
    bench_stale_breakdown
);
criterion_main!(benches);
