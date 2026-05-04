use std::hint::black_box;
use std::time::Duration;

use cachelog::CacheLogConfig;
use cachelog::CacheLogMap;
use criterion::{BenchmarkId, Criterion, Throughput};

pub fn bench_stale_cycle(c: &mut Criterion) {
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
                black_box(stale.read(&1, |_, value, _| *value));
                black_box(stale.insert_clean_if_absent(1, 1));
            });
        },
    );
    group.finish();
}

pub fn bench_stale_breakdown(c: &mut Criterion) {
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
        b.iter(|| black_box(miss_only.read(&1, |_, value, _| *value)));
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
