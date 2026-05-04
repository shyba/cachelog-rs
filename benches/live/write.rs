use std::hint::black_box;
use std::time::Duration;

#[cfg(not(feature = "loom"))]
use ahash::RandomState as AHashState;
#[cfg(not(feature = "loom"))]
use bytes::Bytes;
use cachelog::{CacheLogConfig, CacheLogMap, DirtyWriteMode};
use criterion::{BenchmarkId, Criterion, Throughput};

pub fn bench_dirty_write(c: &mut Criterion) {
    let mut group = c.benchmark_group("live");
    group.measurement_time(Duration::from_secs(3));
    group.throughput(Throughput::Elements(1));
    group.bench_function(BenchmarkId::new("dirty_write", "serial_insert"), |b| {
        let writes = CacheLogMap::<u64, u64>::new(CacheLogConfig::new(1 << 20, 1 << 20, 16));
        let mut key = 0_u64;
        b.iter(|| {
            black_box(writes.low_level().insert_dirty(key, key));
            key = key.wrapping_add(1);
        });
    });
    group.finish();
}

#[cfg(not(feature = "loom"))]
pub fn bench_dirty_write_coalesced_focus(c: &mut Criterion) {
    let mut group = c.benchmark_group("live");
    group.measurement_time(Duration::from_secs(3));
    group.throughput(Throughput::Elements(1));

    group.bench_function(
        BenchmarkId::new("dirty_write_coalesced_focus", "strict_serial_unique"),
        |b| {
            b.iter_custom(|iters| {
                let cap = iters as usize + 16;
                let map = CacheLogMap::<Vec<u8>, u64>::new(CacheLogConfig::new(cap, cap, 16));
                let start = std::time::Instant::now();
                for seq in 0..iters {
                    let key = format!("uniq:{seq:016}").into_bytes();
                    black_box(map.low_level().insert_dirty(key, seq));
                }
                start.elapsed()
            })
        },
    );

    group.bench_function(
        BenchmarkId::new("dirty_write_coalesced_focus", "coalesced_serial_unique"),
        |b| {
            b.iter_custom(|iters| {
                let cap = iters as usize + 16;
                let cfg = CacheLogConfig::new(cap, cap, 16)
                    .with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
                let map = CacheLogMap::<Vec<u8>, u64>::new(cfg);
                let start = std::time::Instant::now();
                for seq in 0..iters {
                    let key = format!("uniq:{seq:016}").into_bytes();
                    black_box(map.low_level().insert_dirty(key, seq));
                }
                start.elapsed()
            })
        },
    );

    group.bench_function(
        BenchmarkId::new(
            "dirty_write_coalesced_focus",
            "coalesced_serial_unique_ahash",
        ),
        |b| {
            b.iter_custom(|iters| {
                let cap = iters as usize + 16;
                let cfg = CacheLogConfig::new(cap, cap, 16)
                    .with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
                let map =
                    CacheLogMap::<Vec<u8>, u64, AHashState>::with_hasher(cfg, AHashState::new());
                let start = std::time::Instant::now();
                for seq in 0..iters {
                    let key = format!("uniq:{seq:016}").into_bytes();
                    black_box(map.low_level().insert_dirty(key, seq));
                }
                start.elapsed()
            })
        },
    );

    group.throughput(Throughput::Elements(1_000));
    group.bench_function(
        BenchmarkId::new(
            "dirty_write_coalesced_focus",
            "strict_batch_unique_1k_without_ids",
        ),
        |b| {
            b.iter_custom(|iters| {
                let writes = iters as usize * 1_000;
                let cap = writes + 16;
                let map = CacheLogMap::<Vec<u8>, u64>::new(CacheLogConfig::new(cap, cap, 16));
                let start = std::time::Instant::now();
                let mut seq = 0_u64;
                for _ in 0..iters {
                    let mut batch = Vec::with_capacity(1_000);
                    for _ in 0..1_000 {
                        let value = seq;
                        batch.push((format!("uniq:{value:016}").into_bytes(), value));
                        seq = seq.wrapping_add(1);
                    }
                    black_box(map.low_level().insert_dirty_batch_without_ids(batch));
                }
                start.elapsed()
            })
        },
    );
    group.bench_function(
        BenchmarkId::new(
            "dirty_write_coalesced_focus",
            "coalesced_batch_unique_1k_without_ids",
        ),
        |b| {
            b.iter_custom(|iters| {
                let writes = iters as usize * 1_000;
                let cap = writes + 16;
                let cfg = CacheLogConfig::new(cap, cap, 16)
                    .with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
                let map = CacheLogMap::<Vec<u8>, u64>::new(cfg);
                let start = std::time::Instant::now();
                let mut seq = 0_u64;
                for _ in 0..iters {
                    let mut batch = Vec::with_capacity(1_000);
                    for _ in 0..1_000 {
                        let value = seq;
                        batch.push((format!("uniq:{value:016}").into_bytes(), value));
                        seq = seq.wrapping_add(1);
                    }
                    black_box(map.low_level().insert_dirty_batch_without_ids(batch));
                }
                start.elapsed()
            })
        },
    );
    group.bench_function(
        BenchmarkId::new(
            "dirty_write_coalesced_focus",
            "coalesced_batch_unique_1k_without_ids_ahash",
        ),
        |b| {
            b.iter_custom(|iters| {
                let writes = iters as usize * 1_000;
                let cap = writes + 16;
                let cfg = CacheLogConfig::new(cap, cap, 16)
                    .with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
                let map =
                    CacheLogMap::<Vec<u8>, u64, AHashState>::with_hasher(cfg, AHashState::new());
                let start = std::time::Instant::now();
                let mut seq = 0_u64;
                for _ in 0..iters {
                    let mut batch = Vec::with_capacity(1_000);
                    for _ in 0..1_000 {
                        let value = seq;
                        batch.push((format!("uniq:{value:016}").into_bytes(), value));
                        seq = seq.wrapping_add(1);
                    }
                    black_box(map.low_level().insert_dirty_batch_without_ids(batch));
                }
                start.elapsed()
            })
        },
    );
    group.bench_function(
        BenchmarkId::new(
            "dirty_write_coalesced_focus",
            "strict_batch_repeated_1k_without_ids",
        ),
        |b| {
            b.iter_custom(|iters| {
                let writes = iters as usize * 1_000;
                let cap = writes + 16;
                let map = CacheLogMap::<Vec<u8>, u64>::new(CacheLogConfig::new(cap, cap, 16));
                let keys = (0_u64..64)
                    .map(|i| format!("hot:{i:04}").into_bytes())
                    .collect::<Vec<_>>();
                let start = std::time::Instant::now();
                let mut seq = 0_u64;
                for _ in 0..iters {
                    let mut batch = Vec::with_capacity(1_000);
                    for i in 0..1_000 {
                        let value = seq;
                        batch.push((keys[i % keys.len()].clone(), value));
                        seq = seq.wrapping_add(1);
                    }
                    black_box(map.low_level().insert_dirty_batch_without_ids(batch));
                }
                start.elapsed()
            })
        },
    );
    group.bench_function(
        BenchmarkId::new(
            "dirty_write_coalesced_focus",
            "coalesced_batch_repeated_1k_without_ids",
        ),
        |b| {
            b.iter_custom(|iters| {
                let writes = iters as usize * 1_000;
                let cap = writes + 16;
                let cfg = CacheLogConfig::new(cap, cap, 16)
                    .with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
                let map = CacheLogMap::<Vec<u8>, u64>::new(cfg);
                let keys = (0_u64..64)
                    .map(|i| format!("hot:{i:04}").into_bytes())
                    .collect::<Vec<_>>();
                let start = std::time::Instant::now();
                let mut seq = 0_u64;
                for _ in 0..iters {
                    let mut batch = Vec::with_capacity(1_000);
                    for i in 0..1_000 {
                        let value = seq;
                        batch.push((keys[i % keys.len()].clone(), value));
                        seq = seq.wrapping_add(1);
                    }
                    black_box(map.low_level().insert_dirty_batch_without_ids(batch));
                }
                start.elapsed()
            })
        },
    );
    group.bench_function(
        BenchmarkId::new(
            "dirty_write_coalesced_focus",
            "coalesced_borrowed_batch_unique_1k_without_ids_bytes",
        ),
        |b| {
            b.iter_custom(|iters| {
                let writes = iters as usize * 1_000;
                let cap = writes + 16;
                let cfg = CacheLogConfig::new(cap, cap, 16)
                    .with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
                let map =
                    CacheLogMap::<Vec<u8>, Bytes, AHashState>::with_hasher(cfg, AHashState::new());
                let batch = (0_u64..1_000)
                    .map(|i| {
                        (
                            format!("uniq:{i:016}").into_bytes(),
                            Bytes::from(format!("value:{i:016}").into_bytes()),
                        )
                    })
                    .collect::<Vec<_>>();
                let start = std::time::Instant::now();
                for _ in 0..iters {
                    black_box(
                        map.low_level()
                            .insert_dirty_batch_without_ids(batch.clone()),
                    );
                }
                start.elapsed()
            })
        },
    );

    group.finish();
}

pub fn bench_dirty_write_batch_modes(c: &mut Criterion) {
    let mut group = c.benchmark_group("live");
    group.measurement_time(Duration::from_secs(3));
    group.throughput(Throughput::Elements(1_000));

    group.bench_function(
        BenchmarkId::new("dirty_write_batch", "strict_repeated_1k"),
        |b| {
            let map = CacheLogMap::<Vec<u8>, u64>::new(CacheLogConfig::new(1 << 15, 1 << 15, 16));
            let keys = (0_u64..64)
                .map(|i| format!("hot:{i:04}").into_bytes())
                .collect::<Vec<_>>();
            let mut seq = 0_u64;
            b.iter(|| {
                let mut batch = Vec::with_capacity(1_000);
                for i in 0..1_000 {
                    let key = keys[i % keys.len()].clone();
                    batch.push((key, seq));
                    seq = seq.wrapping_add(1);
                }
                black_box(map.low_level().insert_dirty_batch(batch).len());
            });
        },
    );

    group.bench_function(
        BenchmarkId::new("dirty_write_batch", "coalesced_repeated_1k"),
        |b| {
            let cfg = CacheLogConfig::new(1 << 15, 1 << 15, 16)
                .with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
            let map = CacheLogMap::<Vec<u8>, u64>::new(cfg);
            let keys = (0_u64..64)
                .map(|i| format!("hot:{i:04}").into_bytes())
                .collect::<Vec<_>>();
            let mut seq = 0_u64;
            b.iter(|| {
                let mut batch = Vec::with_capacity(1_000);
                for i in 0..1_000 {
                    let key = keys[i % keys.len()].clone();
                    batch.push((key, seq));
                    seq = seq.wrapping_add(1);
                }
                black_box(map.low_level().insert_dirty_batch(batch).len());
            });
        },
    );

    group.bench_function(
        BenchmarkId::new("dirty_write_batch", "strict_unique_1k"),
        |b| {
            let map = CacheLogMap::<Vec<u8>, u64>::new(CacheLogConfig::new(1 << 15, 1 << 15, 16));
            let mut seq = 0_u64;
            b.iter(|| {
                let mut batch = Vec::with_capacity(1_000);
                for _ in 0..1_000 {
                    let key = format!("uniq:{seq:016}").into_bytes();
                    batch.push((key, seq));
                    seq = seq.wrapping_add(1);
                }
                black_box(map.low_level().insert_dirty_batch(batch).len());
            });
        },
    );

    group.bench_function(
        BenchmarkId::new("dirty_write_batch", "coalesced_unique_1k"),
        |b| {
            let cfg = CacheLogConfig::new(1 << 15, 1 << 15, 16)
                .with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
            let map = CacheLogMap::<Vec<u8>, u64>::new(cfg);
            let mut seq = 0_u64;
            b.iter(|| {
                let mut batch = Vec::with_capacity(1_000);
                for _ in 0..1_000 {
                    let key = format!("uniq:{seq:016}").into_bytes();
                    batch.push((key, seq));
                    seq = seq.wrapping_add(1);
                }
                black_box(map.low_level().insert_dirty_batch(batch).len());
            });
        },
    );

    group.finish();
}
