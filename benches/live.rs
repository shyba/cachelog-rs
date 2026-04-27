use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

#[cfg(not(feature = "loom"))]
use ahash::RandomState as AHashState;
#[cfg(not(feature = "loom"))]
use bytes::Bytes;
#[cfg(not(feature = "loom"))]
use cachelog::BackgroundFlushConfig;
use cachelog::{BytePrefixMap, CacheLogConfig, CacheLogMap, DirtyWriteMode};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

fn bench_dirty_write(c: &mut Criterion) {
    let mut group = c.benchmark_group("live");
    group.measurement_time(Duration::from_secs(3));
    group.throughput(Throughput::Elements(1));

    group.bench_function(BenchmarkId::new("dirty_write", "serial_insert"), |b| {
        let writes = CacheLogMap::<u64, u64>::new(CacheLogConfig::new(1 << 20, 1 << 20, 16));
        let mut key = 0_u64;
        b.iter(|| {
            let id = writes.low_level().insert_dirty(key, key);
            black_box(id);
            key = key.wrapping_add(1);
        });
    });

    group.finish();
}

#[cfg(not(feature = "loom"))]
fn bench_dirty_write_coalesced_focus(c: &mut Criterion) {
    let mut group = c.benchmark_group("live");
    group.measurement_time(Duration::from_secs(3));
    group.throughput(Throughput::Elements(1));

    #[cfg(not(feature = "loom"))]
    group.bench_function(
        BenchmarkId::new("dirty_write_coalesced_focus", "strict_serial_unique"),
        |b| {
            b.iter_custom(|iters| {
                let cap = iters as usize + 16;
                let map = CacheLogMap::<Vec<u8>, u64>::new(CacheLogConfig::new(cap, cap, 16));
                let start = std::time::Instant::now();
                for seq in 0..iters {
                    let key = format!("uniq:{seq:016}").into_bytes();
                    let id = map.low_level().insert_dirty(key, seq);
                    black_box(id);
                }
                start.elapsed()
            });
        },
    );

    #[cfg(not(feature = "loom"))]
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
                    let id = map.low_level().insert_dirty(key, seq);
                    black_box(id);
                }
                start.elapsed()
            });
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
                    let id = map.low_level().insert_dirty(key, seq);
                    black_box(id);
                }
                start.elapsed()
            });
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
                    let written = map.low_level().insert_dirty_batch_without_ids(batch);
                    black_box(written);
                }
                start.elapsed()
            });
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
                    let written = map.low_level().insert_dirty_batch_without_ids(batch);
                    black_box(written);
                }
                start.elapsed()
            });
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
                    let written = map.low_level().insert_dirty_batch_without_ids(batch);
                    black_box(written);
                }
                start.elapsed()
            });
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
                    let written = map.low_level().insert_dirty_batch_without_ids(batch);
                    black_box(written);
                }
                start.elapsed()
            });
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
                    let written = map.low_level().insert_dirty_batch_without_ids(batch);
                    black_box(written);
                }
                start.elapsed()
            });
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
                let map = CacheLogMap::<Vec<u8>, Bytes>::new(cfg);
                let mut storage = Vec::with_capacity(1_000);
                let start = std::time::Instant::now();
                let mut seq = 0_u64;
                for _ in 0..iters {
                    storage.clear();
                    for _ in 0..1_000 {
                        let value = seq;
                        storage.push((
                            format!("uniq:{value:016}").into_bytes(),
                            format!("value:{value:016}").into_bytes(),
                        ));
                        seq = seq.wrapping_add(1);
                    }
                    let written = map.low_level().insert_dirty_batch_borrowed_without_ids(
                        storage
                            .iter()
                            .map(|(key, value)| (key.as_slice(), value.as_slice())),
                    );
                    black_box(written);
                }
                start.elapsed()
            });
        },
    );

    group.bench_function(
        BenchmarkId::new(
            "dirty_write_coalesced_focus",
            "coalesced_borrowed_batch_unique_1k_without_ids_bytes_ahash",
        ),
        |b| {
            b.iter_custom(|iters| {
                let writes = iters as usize * 1_000;
                let cap = writes + 16;
                let cfg = CacheLogConfig::new(cap, cap, 16)
                    .with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
                let map =
                    CacheLogMap::<Vec<u8>, Bytes, AHashState>::with_hasher(cfg, AHashState::new());
                let mut storage = Vec::with_capacity(1_000);
                let start = std::time::Instant::now();
                let mut seq = 0_u64;
                for _ in 0..iters {
                    storage.clear();
                    for _ in 0..1_000 {
                        let value = seq;
                        storage.push((
                            format!("uniq:{value:016}").into_bytes(),
                            format!("value:{value:016}").into_bytes(),
                        ));
                        seq = seq.wrapping_add(1);
                    }
                    let written = map.low_level().insert_dirty_batch_borrowed_without_ids(
                        storage
                            .iter()
                            .map(|(key, value)| (key.as_slice(), value.as_slice())),
                    );
                    black_box(written);
                }
                start.elapsed()
            });
        },
    );

    group.bench_function(
        BenchmarkId::new(
            "dirty_write_coalesced_focus",
            "coalesced_owned_bytes_batch_unique_1k_without_ids_ahash",
        ),
        |b| {
            b.iter_custom(|iters| {
                let writes = iters as usize * 1_000;
                let cap = writes + 16;
                let cfg = CacheLogConfig::new(cap, cap, 16)
                    .with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
                let map =
                    CacheLogMap::<Vec<u8>, Bytes, AHashState>::with_hasher(cfg, AHashState::new());
                let start = std::time::Instant::now();
                let mut seq = 0_u64;
                for _ in 0..iters {
                    let mut batch = Vec::with_capacity(1_000);
                    for _ in 0..1_000 {
                        let value = seq;
                        batch.push((
                            format!("uniq:{value:016}").into_bytes(),
                            Bytes::from(format!("value:{value:016}").into_bytes()),
                        ));
                        seq = seq.wrapping_add(1);
                    }
                    let written = map.low_level().insert_dirty_batch_without_ids(batch);
                    black_box(written);
                }
                start.elapsed()
            });
        },
    );

    group.bench_function(
        BenchmarkId::new(
            "dirty_write_coalesced_focus",
            "coalesced_borrowed_batch_unique_1k_without_ids_bytes_ahash_prebuilt",
        ),
        |b| {
            let storage = (0_u64..1_000)
                .map(|value| {
                    (
                        format!("uniq:{value:016}").into_bytes(),
                        format!("value:{value:016}").into_bytes(),
                    )
                })
                .collect::<Vec<_>>();
            b.iter_custom(|iters| {
                let writes = iters as usize * 1_000;
                let cap = writes + 16;
                let cfg = CacheLogConfig::new(cap, cap, 16)
                    .with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
                let map =
                    CacheLogMap::<Vec<u8>, Bytes, AHashState>::with_hasher(cfg, AHashState::new());
                let start = std::time::Instant::now();
                for _ in 0..iters {
                    let written = map.low_level().insert_dirty_batch_borrowed_without_ids(
                        storage
                            .iter()
                            .map(|(key, value)| (key.as_slice(), value.as_slice())),
                    );
                    black_box(written);
                }
                start.elapsed()
            });
        },
    );

    group.bench_function(
        BenchmarkId::new(
            "dirty_write_coalesced_focus",
            "coalesced_borrowed_batch_unique_1k_without_ids_vec_ahash_prebuilt",
        ),
        |b| {
            let storage = (0_u64..1_000)
                .map(|value| {
                    (
                        format!("uniq:{value:016}").into_bytes(),
                        format!("value:{value:016}").into_bytes(),
                    )
                })
                .collect::<Vec<_>>();
            b.iter_custom(|iters| {
                let writes = iters as usize * 1_000;
                let cap = writes + 16;
                let cfg = CacheLogConfig::new(cap, cap, 16)
                    .with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
                let map = CacheLogMap::<Vec<u8>, Vec<u8>, AHashState>::with_hasher(
                    cfg,
                    AHashState::new(),
                );
                let start = std::time::Instant::now();
                for _ in 0..iters {
                    let written = map.low_level().insert_dirty_batch_borrowed_without_ids(
                        storage
                            .iter()
                            .map(|(key, value)| (key.as_slice(), value.as_slice())),
                    );
                    black_box(written);
                }
                start.elapsed()
            });
        },
    );

    group.bench_function(
        BenchmarkId::new(
            "dirty_write_coalesced_focus",
            "coalesced_borrowed_batch_unique_1k_without_ids_bytes_ahash_prebuilt_bgflush",
        ),
        |b| {
            let storage = (0_u64..1_000)
                .map(|value| {
                    (
                        format!("uniq:{value:016}").into_bytes(),
                        format!("value:{value:016}").into_bytes(),
                    )
                })
                .collect::<Vec<_>>();
            b.iter_custom(|iters| {
                let writes = iters as usize * 1_000;
                let cap = writes + 16;
                let cfg = CacheLogConfig::new(cap, cap, 16)
                    .with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
                let map = Arc::new(CacheLogMap::<Vec<u8>, Bytes, AHashState>::with_hasher(
                    cfg,
                    AHashState::new(),
                ));
                let flush = map.clone().start_background_flush(
                    BackgroundFlushConfig::new(1_000, 512, Duration::from_secs(10)),
                    |batch| {
                        black_box(batch.len());
                        Ok(())
                    },
                );
                let start = std::time::Instant::now();
                for _ in 0..iters {
                    let written = map.low_level().insert_dirty_batch_borrowed_without_ids(
                        storage
                            .iter()
                            .map(|(key, value)| (key.as_slice(), value.as_slice())),
                    );
                    flush
                        .note_writes(written)
                        .expect("background flush should stay running");
                    black_box(written);
                }
                let elapsed = start.elapsed();
                flush
                    .flush_sync()
                    .expect("background flush should drain benchmark backlog");
                flush
                    .shutdown()
                    .expect("background flush should shut down cleanly");
                elapsed
            });
        },
    );

    group.bench_function(
        BenchmarkId::new(
            "dirty_write_coalesced_focus",
            "coalesced_borrowed_batch_unique_1k_without_ids_vec_ahash_prebuilt_bgflush",
        ),
        |b| {
            let storage = (0_u64..1_000)
                .map(|value| {
                    (
                        format!("uniq:{value:016}").into_bytes(),
                        format!("value:{value:016}").into_bytes(),
                    )
                })
                .collect::<Vec<_>>();
            b.iter_custom(|iters| {
                let writes = iters as usize * 1_000;
                let cap = writes + 16;
                let cfg = CacheLogConfig::new(cap, cap, 16)
                    .with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
                let map = Arc::new(CacheLogMap::<Vec<u8>, Vec<u8>, AHashState>::with_hasher(
                    cfg,
                    AHashState::new(),
                ));
                let flush = map.clone().start_background_flush(
                    BackgroundFlushConfig::new(1_000, 512, Duration::from_secs(10)),
                    |batch| {
                        black_box(batch.len());
                        Ok(())
                    },
                );
                let start = std::time::Instant::now();
                for _ in 0..iters {
                    let written = map.low_level().insert_dirty_batch_borrowed_without_ids(
                        storage
                            .iter()
                            .map(|(key, value)| (key.as_slice(), value.as_slice())),
                    );
                    flush
                        .note_writes(written)
                        .expect("background flush should stay running");
                    black_box(written);
                }
                let elapsed = start.elapsed();
                flush
                    .flush_sync()
                    .expect("background flush should drain benchmark backlog");
                flush
                    .shutdown()
                    .expect("background flush should shut down cleanly");
                elapsed
            });
        },
    );

    group.bench_function(
        BenchmarkId::new(
            "dirty_write_coalesced_focus",
            "coalesced_owned_bytes_batch_unique_1k_without_ids_ahash_prebuilt",
        ),
        |b| {
            let batch = (0_u64..1_000)
                .map(|value| {
                    (
                        format!("uniq:{value:016}").into_bytes(),
                        Bytes::from(format!("value:{value:016}").into_bytes()),
                    )
                })
                .collect::<Vec<_>>();
            b.iter_custom(|iters| {
                let writes = iters as usize * 1_000;
                let cap = writes + 16;
                let cfg = CacheLogConfig::new(cap, cap, 16)
                    .with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
                let map =
                    CacheLogMap::<Vec<u8>, Bytes, AHashState>::with_hasher(cfg, AHashState::new());
                let start = std::time::Instant::now();
                for _ in 0..iters {
                    let written = map
                        .low_level()
                        .insert_dirty_batch_without_ids(batch.clone());
                    black_box(written);
                }
                start.elapsed()
            });
        },
    );

    group.bench_function(
        BenchmarkId::new(
            "dirty_write_coalesced_focus",
            "coalesced_borrowed_batch_repeated_1k_without_ids_bytes",
        ),
        |b| {
            b.iter_custom(|iters| {
                let writes = iters as usize * 1_000;
                let cap = writes + 16;
                let cfg = CacheLogConfig::new(cap, cap, 16)
                    .with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
                let map = CacheLogMap::<Vec<u8>, Bytes>::new(cfg);
                let hot_keys = (0_u64..64)
                    .map(|i| format!("hot:{i:04}").into_bytes())
                    .collect::<Vec<_>>();
                let mut values = Vec::with_capacity(1_000);
                let start = std::time::Instant::now();
                let mut seq = 0_u64;
                for _ in 0..iters {
                    values.clear();
                    for i in 0..1_000 {
                        let value = seq;
                        values.push(format!("value:{value:016}").into_bytes());
                        seq = seq.wrapping_add(1);
                        let _ = i;
                    }
                    let written = map.low_level().insert_dirty_batch_borrowed_without_ids(
                        values.iter().enumerate().map(|(i, value)| {
                            (hot_keys[i % hot_keys.len()].as_slice(), value.as_slice())
                        }),
                    );
                    black_box(written);
                }
                start.elapsed()
            });
        },
    );

    group.bench_function(
        BenchmarkId::new(
            "dirty_write_coalesced_focus",
            "coalesced_borrowed_batch_repeated_1k_without_ids_bytes_ahash",
        ),
        |b| {
            b.iter_custom(|iters| {
                let writes = iters as usize * 1_000;
                let cap = writes + 16;
                let cfg = CacheLogConfig::new(cap, cap, 16)
                    .with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
                let map =
                    CacheLogMap::<Vec<u8>, Bytes, AHashState>::with_hasher(cfg, AHashState::new());
                let hot_keys = (0_u64..64)
                    .map(|i| format!("hot:{i:04}").into_bytes())
                    .collect::<Vec<_>>();
                let mut values = Vec::with_capacity(1_000);
                let start = std::time::Instant::now();
                let mut seq = 0_u64;
                for _ in 0..iters {
                    values.clear();
                    for _ in 0..1_000 {
                        let value = seq;
                        values.push(format!("value:{value:016}").into_bytes());
                        seq = seq.wrapping_add(1);
                    }
                    let written = map.low_level().insert_dirty_batch_borrowed_without_ids(
                        values.iter().enumerate().map(|(i, value)| {
                            (hot_keys[i % hot_keys.len()].as_slice(), value.as_slice())
                        }),
                    );
                    black_box(written);
                }
                start.elapsed()
            });
        },
    );

    group.bench_function(
        BenchmarkId::new(
            "dirty_write_coalesced_focus",
            "coalesced_owned_bytes_batch_repeated_1k_without_ids_ahash",
        ),
        |b| {
            b.iter_custom(|iters| {
                let writes = iters as usize * 1_000;
                let cap = writes + 16;
                let cfg = CacheLogConfig::new(cap, cap, 16)
                    .with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
                let map =
                    CacheLogMap::<Vec<u8>, Bytes, AHashState>::with_hasher(cfg, AHashState::new());
                let hot_keys = (0_u64..64)
                    .map(|i| format!("hot:{i:04}").into_bytes())
                    .collect::<Vec<_>>();
                let start = std::time::Instant::now();
                let mut seq = 0_u64;
                for _ in 0..iters {
                    let mut batch = Vec::with_capacity(1_000);
                    for i in 0..1_000 {
                        let value = seq;
                        batch.push((
                            hot_keys[i % hot_keys.len()].clone(),
                            Bytes::from(format!("value:{value:016}").into_bytes()),
                        ));
                        seq = seq.wrapping_add(1);
                    }
                    let written = map.low_level().insert_dirty_batch_without_ids(batch);
                    black_box(written);
                }
                start.elapsed()
            });
        },
    );

    group.bench_function(
        BenchmarkId::new(
            "dirty_write_coalesced_focus",
            "coalesced_borrowed_batch_repeated_1k_without_ids_bytes_ahash_prebuilt",
        ),
        |b| {
            let hot_keys = (0_u64..64)
                .map(|i| format!("hot:{i:04}").into_bytes())
                .collect::<Vec<_>>();
            let values = (0_u64..1_000)
                .map(|value| format!("value:{value:016}").into_bytes())
                .collect::<Vec<_>>();
            b.iter_custom(|iters| {
                let writes = iters as usize * 1_000;
                let cap = writes + 16;
                let cfg = CacheLogConfig::new(cap, cap, 16)
                    .with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
                let map =
                    CacheLogMap::<Vec<u8>, Bytes, AHashState>::with_hasher(cfg, AHashState::new());
                let start = std::time::Instant::now();
                for _ in 0..iters {
                    let written = map.low_level().insert_dirty_batch_borrowed_without_ids(
                        values.iter().enumerate().map(|(i, value)| {
                            (hot_keys[i % hot_keys.len()].as_slice(), value.as_slice())
                        }),
                    );
                    black_box(written);
                }
                start.elapsed()
            });
        },
    );

    group.bench_function(
        BenchmarkId::new(
            "dirty_write_coalesced_focus",
            "coalesced_owned_bytes_batch_repeated_1k_without_ids_ahash_prebuilt",
        ),
        |b| {
            let hot_keys = (0_u64..64)
                .map(|i| format!("hot:{i:04}").into_bytes())
                .collect::<Vec<_>>();
            let batch = (0..1_000)
                .map(|i| {
                    (
                        hot_keys[i % hot_keys.len()].clone(),
                        Bytes::from(format!("value:{i:016}").into_bytes()),
                    )
                })
                .collect::<Vec<_>>();
            b.iter_custom(|iters| {
                let writes = iters as usize * 1_000;
                let cap = writes + 16;
                let cfg = CacheLogConfig::new(cap, cap, 16)
                    .with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
                let map =
                    CacheLogMap::<Vec<u8>, Bytes, AHashState>::with_hasher(cfg, AHashState::new());
                let start = std::time::Instant::now();
                for _ in 0..iters {
                    let written = map
                        .low_level()
                        .insert_dirty_batch_without_ids(batch.clone());
                    black_box(written);
                }
                start.elapsed()
            });
        },
    );

    group.finish();
}

fn bench_dirty_read(c: &mut Criterion) {
    let mut group = c.benchmark_group("live");
    group.measurement_time(Duration::from_secs(3));
    group.throughput(Throughput::Elements(1));

    let dirty_reads = CacheLogMap::<u64, u64>::new(CacheLogConfig::new(1024, 1024, 1024));
    dirty_reads.low_level().insert_dirty(7, 77);

    group.bench_function(BenchmarkId::new("dirty_read", "serial_hit"), |b| {
        b.iter(|| {
            let result = dirty_reads.read(&7, |_, value, _| *value).unwrap();
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
    string_reads
        .low_level()
        .insert_dirty("alpha".to_owned(), 77);
    let owned_string = "alpha".to_owned();

    group.bench_function(BenchmarkId::new("dirty_read_string", "owned_lookup"), |b| {
        b.iter(|| {
            let result = string_reads
                .read(&owned_string, |_, value, _| *value)
                .unwrap();
            black_box(result);
        });
    });

    group.bench_function(
        BenchmarkId::new("dirty_read_string", "borrowed_lookup"),
        |b| {
            b.iter(|| {
                let result = string_reads.read("alpha", |_, value, _| *value).unwrap();
                black_box(result);
            });
        },
    );

    let byte_reads = CacheLogMap::<Vec<u8>, u64>::new(CacheLogConfig::new(1024, 1024, 1024));
    byte_reads.low_level().insert_dirty(b"alpha".to_vec(), 88);

    group.bench_function(
        BenchmarkId::new("dirty_read_bytes", "owned_lookup_with_clone"),
        |b| {
            b.iter(|| {
                let key = b"alpha".to_vec();
                let result = byte_reads.read(&key, |_, value, _| *value).unwrap();
                black_box(result);
            });
        },
    );

    group.bench_function(
        BenchmarkId::new("dirty_read_bytes", "borrowed_lookup"),
        |b| {
            b.iter(|| {
                let result = byte_reads
                    .read(b"alpha".as_slice(), |_, value, _| *value)
                    .unwrap();
                black_box(result);
            });
        },
    );

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
                black_box(stale.read(&1, |_, value, _| *value));
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
            black_box(miss_only.read(&1, |_, value, _| *value));
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

fn build_prefix_key_pool() -> Arc<Vec<Vec<u8>>> {
    Arc::new(
        (0_u64..5_000)
            .map(|i| {
                if i % 5 == 0 {
                    format!("ab:{i:08}").into_bytes()
                } else {
                    format!("zz:{i:08}").into_bytes()
                }
            })
            .collect::<Vec<_>>(),
    )
}

fn bench_dirty_read_under_write(c: &mut Criterion) {
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

            b.iter(|| {
                let result = map.read(&7, |_, value, _| *value).unwrap();
                black_box(result);
            });

            stop.store(true, Ordering::Release);
            writer.join().unwrap();
        },
    );

    group.finish();
}

fn bench_mixed_rw(c: &mut Criterion) {
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

fn bench_prefix_list_with_advance(c: &mut Criterion) {
    let mut group = c.benchmark_group("live");
    group.measurement_time(Duration::from_secs(3));
    group.throughput(Throughput::Elements(1));

    let key_pool = build_prefix_key_pool();
    let map = Arc::new(BytePrefixMap::<u64>::new(CacheLogConfig::new(
        1 << 15,
        1 << 15,
        1 << 15,
    )));
    for (i, key) in key_pool.iter().enumerate() {
        map.insert_dirty(key.clone(), i as u64);
    }
    let _ = map.advance_trie(usize::MAX);

    group.bench_function(BenchmarkId::new("prefix_list_indexed", "serial_5k"), |b| {
        b.iter(|| {
            let rows = map.list_prefix(b"ab:", |_, value, _| *value, 5_000);
            black_box(rows.len());
        });
    });

    group.bench_function(
        BenchmarkId::new("prefix_list_indexed", "under_concurrent_write_5k"),
        |b| {
            let stop = Arc::new(AtomicBool::new(false));
            let writer_map = Arc::clone(&map);
            let writer_stop = Arc::clone(&stop);
            let writer_keys = Arc::clone(&key_pool);
            let writer = thread::spawn(move || {
                let mut i = 0_u64;
                while !writer_stop.load(Ordering::Acquire) {
                    let idx = (i as usize) % writer_keys.len();
                    let key = writer_keys[idx].clone();
                    black_box(writer_map.insert_dirty(key, i));
                    i = i.wrapping_add(1);
                }
            });

            let advance_map = Arc::clone(&map);
            let advance_stop = Arc::clone(&stop);
            let advancer = thread::spawn(move || {
                while !advance_stop.load(Ordering::Acquire) {
                    let _ = advance_map.advance_trie(256);
                    std::hint::spin_loop();
                }
            });

            b.iter(|| {
                let rows = map.list_prefix(b"ab:", |_, value, _| *value, 5_000);
                black_box(rows.len());
            });

            stop.store(true, Ordering::Release);
            writer.join().unwrap();
            advancer.join().unwrap();
        },
    );

    group.bench_function(
        BenchmarkId::new("prefix_list_indexed", "mixed_with_point_reads_5k"),
        |b| {
            let stop = Arc::new(AtomicBool::new(false));
            let writer_map = Arc::clone(&map);
            let writer_stop = Arc::clone(&stop);
            let writer_keys = Arc::clone(&key_pool);
            let writer = thread::spawn(move || {
                let mut i = 0_u64;
                while !writer_stop.load(Ordering::Acquire) {
                    let idx = (i as usize) % writer_keys.len();
                    let key = writer_keys[idx].clone();
                    black_box(writer_map.insert_dirty(key, i));
                    i = i.wrapping_add(1);
                }
            });

            let advance_map = Arc::clone(&map);
            let advance_stop = Arc::clone(&stop);
            let advancer = thread::spawn(move || {
                while !advance_stop.load(Ordering::Acquire) {
                    let _ = advance_map.advance_trie(256);
                    std::hint::spin_loop();
                }
            });

            let read_map = Arc::clone(&map);
            let read_stop = Arc::clone(&stop);
            let reader = thread::spawn(move || {
                while !read_stop.load(Ordering::Acquire) {
                    black_box(read_map.read(b"ab:00000000", |_, value, _| *value));
                }
            });

            b.iter(|| {
                let rows = map.list_prefix(b"ab:", |_, value, _| *value, 5_000);
                black_box(rows.len());
                let read = map.read(b"ab:00000000", |_, value, _| *value);
                black_box(read);
            });

            stop.store(true, Ordering::Release);
            writer.join().unwrap();
            advancer.join().unwrap();
            reader.join().unwrap();
        },
    );

    group.finish();
}

fn bench_dirty_write_batch_modes(c: &mut Criterion) {
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
                let ids = map.low_level().insert_dirty_batch(batch);
                black_box(ids.len());
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
                let ids = map.low_level().insert_dirty_batch(batch);
                black_box(ids.len());
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
                let ids = map.low_level().insert_dirty_batch(batch);
                black_box(ids.len());
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
                let ids = map.low_level().insert_dirty_batch(batch);
                black_box(ids.len());
            });
        },
    );

    group.finish();
}

#[cfg(not(feature = "loom"))]
fn bench_dirty_write_arc_value(c: &mut Criterion) {
    let mut group = c.benchmark_group("live");
    group.measurement_time(Duration::from_secs(3));
    group.throughput(Throughput::Elements(1));

    let cfg = CacheLogConfig::new(1 << 15, 1 << 15, 16);
    let vec_map = CacheLogMap::<Vec<u8>, Vec<u8>>::new(cfg);
    let arc_map = CacheLogMap::<Vec<u8>, Arc<Vec<u8>>>::new(cfg);
    let arc_slice_map = CacheLogMap::<Vec<u8>, Arc<[u8]>>::new(cfg);
    let bytes_map = CacheLogMap::<Vec<u8>, Bytes>::new(cfg);
    let bytes_map_ahash =
        CacheLogMap::<Vec<u8>, Bytes, AHashState>::with_hasher(cfg, AHashState::new());
    let keys = (0_u64..1024)
        .map(|i| format!("arc:{i:04}").into_bytes())
        .collect::<Vec<_>>();
    let payload_4k = vec![0xA5_u8; 4096];
    let payload_9b = vec![0xB7_u8; 9];
    let preowned_4k: Arc<[u8]> = Arc::from(payload_4k.clone());
    let preowned_9b: Arc<[u8]> = Arc::from(payload_9b.clone());
    let preowned_bytes_4k = Bytes::from(payload_4k.clone());
    let preowned_bytes_9b = Bytes::from(payload_9b.clone());

    group.bench_function(
        BenchmarkId::new("dirty_write_bytes", "vec_borrowed_9b"),
        |b| {
            let mut i = 0_usize;
            b.iter(|| {
                let key = &keys[i % keys.len()];
                let id = vec_map.low_level().insert_dirty_borrowed(key, &payload_9b);
                black_box(id);
                i = i.wrapping_add(1);
            });
        },
    );

    group.bench_function(
        BenchmarkId::new("dirty_write_bytes", "arc_borrowed_9b"),
        |b| {
            let mut i = 0_usize;
            b.iter(|| {
                let key = &keys[i % keys.len()];
                let id = arc_map.low_level().insert_dirty_borrowed(key, &payload_9b);
                black_box(id);
                i = i.wrapping_add(1);
            });
        },
    );

    group.bench_function(
        BenchmarkId::new("dirty_write_bytes", "arc_slice_borrowed_9b"),
        |b| {
            let mut i = 0_usize;
            b.iter(|| {
                let key = &keys[i % keys.len()];
                let id = arc_slice_map
                    .low_level()
                    .insert_dirty_borrowed(key, &payload_9b);
                black_box(id);
                i = i.wrapping_add(1);
            });
        },
    );

    group.bench_function(
        BenchmarkId::new("dirty_write_bytes", "arc_slice_preowned_9b"),
        |b| {
            let mut i = 0_usize;
            b.iter(|| {
                let key = keys[i % keys.len()].clone();
                let id = arc_slice_map
                    .low_level()
                    .insert_dirty_preowned(key, preowned_9b.clone());
                black_box(id);
                i = i.wrapping_add(1);
            });
        },
    );

    group.bench_function(
        BenchmarkId::new("dirty_write_bytes", "bytes_borrowed_9b"),
        |b| {
            let mut i = 0_usize;
            b.iter(|| {
                let key = &keys[i % keys.len()];
                let id = bytes_map
                    .low_level()
                    .insert_dirty_borrowed(key, &payload_9b);
                black_box(id);
                i = i.wrapping_add(1);
            });
        },
    );

    group.bench_function(
        BenchmarkId::new("dirty_write_bytes", "bytes_preowned_9b"),
        |b| {
            let mut i = 0_usize;
            b.iter(|| {
                let key = keys[i % keys.len()].clone();
                let id = bytes_map
                    .low_level()
                    .insert_dirty_preowned(key, preowned_bytes_9b.clone());
                black_box(id);
                i = i.wrapping_add(1);
            });
        },
    );

    group.bench_function(
        BenchmarkId::new("dirty_write_bytes", "bytes_borrowed_9b_ahash"),
        |b| {
            let mut i = 0_usize;
            b.iter(|| {
                let key = &keys[i % keys.len()];
                let id = bytes_map_ahash
                    .low_level()
                    .insert_dirty_borrowed(key, &payload_9b);
                black_box(id);
                i = i.wrapping_add(1);
            });
        },
    );

    group.bench_function(
        BenchmarkId::new("dirty_write_bytes", "vec_borrowed_4k"),
        |b| {
            let mut i = 0_usize;
            b.iter(|| {
                let key = &keys[i % keys.len()];
                let id = vec_map.low_level().insert_dirty_borrowed(key, &payload_4k);
                black_box(id);
                i = i.wrapping_add(1);
            });
        },
    );

    group.bench_function(
        BenchmarkId::new("dirty_write_bytes", "arc_borrowed_4k"),
        |b| {
            let mut i = 0_usize;
            b.iter(|| {
                let key = &keys[i % keys.len()];
                let id = arc_map.low_level().insert_dirty_borrowed(key, &payload_4k);
                black_box(id);
                i = i.wrapping_add(1);
            });
        },
    );

    group.bench_function(
        BenchmarkId::new("dirty_write_bytes", "arc_slice_borrowed_4k"),
        |b| {
            let mut i = 0_usize;
            b.iter(|| {
                let key = &keys[i % keys.len()];
                let id = arc_slice_map
                    .low_level()
                    .insert_dirty_borrowed(key, &payload_4k);
                black_box(id);
                i = i.wrapping_add(1);
            });
        },
    );

    group.bench_function(
        BenchmarkId::new("dirty_write_bytes", "arc_slice_preowned_4k"),
        |b| {
            let mut i = 0_usize;
            b.iter(|| {
                let key = keys[i % keys.len()].clone();
                let id = arc_slice_map
                    .low_level()
                    .insert_dirty_preowned(key, preowned_4k.clone());
                black_box(id);
                i = i.wrapping_add(1);
            });
        },
    );

    group.bench_function(
        BenchmarkId::new("dirty_write_bytes", "bytes_borrowed_4k"),
        |b| {
            let mut i = 0_usize;
            b.iter(|| {
                let key = &keys[i % keys.len()];
                let id = bytes_map
                    .low_level()
                    .insert_dirty_borrowed(key, &payload_4k);
                black_box(id);
                i = i.wrapping_add(1);
            });
        },
    );

    group.bench_function(
        BenchmarkId::new("dirty_write_bytes", "bytes_preowned_4k"),
        |b| {
            let mut i = 0_usize;
            b.iter(|| {
                let key = keys[i % keys.len()].clone();
                let id = bytes_map
                    .low_level()
                    .insert_dirty_preowned(key, preowned_bytes_4k.clone());
                black_box(id);
                i = i.wrapping_add(1);
            });
        },
    );

    group.bench_function(
        BenchmarkId::new("dirty_write_bytes", "bytes_borrowed_4k_ahash"),
        |b| {
            let mut i = 0_usize;
            b.iter(|| {
                let key = &keys[i % keys.len()];
                let id = bytes_map_ahash
                    .low_level()
                    .insert_dirty_borrowed(key, &payload_4k);
                black_box(id);
                i = i.wrapping_add(1);
            });
        },
    );

    group.finish();
}

#[cfg(not(feature = "loom"))]
criterion_group!(
    benches,
    bench_dirty_write,
    bench_dirty_write_coalesced_focus,
    bench_dirty_read,
    bench_dirty_write_arc_value,
    bench_dirty_write_batch_modes,
    bench_borrowed_lookup,
    bench_dirty_read_under_write,
    bench_mixed_rw,
    bench_prefix_list_with_advance,
    bench_stale_cycle,
    bench_stale_breakdown
);

#[cfg(feature = "loom")]
criterion_group!(
    benches,
    bench_dirty_write,
    bench_dirty_read,
    bench_dirty_write_batch_modes,
    bench_borrowed_lookup,
    bench_dirty_read_under_write,
    bench_mixed_rw,
    bench_prefix_list_with_advance,
    bench_stale_cycle,
    bench_stale_breakdown
);
criterion_main!(benches);
