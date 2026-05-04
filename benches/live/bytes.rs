#[cfg(not(feature = "loom"))]
use ahash::RandomState as AHashState;
#[cfg(not(feature = "loom"))]
use bytes::Bytes;
#[cfg(not(feature = "loom"))]
use cachelog::{CacheLogConfig, CacheLogMap};
#[cfg(not(feature = "loom"))]
use criterion::{BenchmarkId, Criterion, Throughput};
#[cfg(not(feature = "loom"))]
use std::hint::black_box;
#[cfg(not(feature = "loom"))]
use std::sync::Arc;
#[cfg(not(feature = "loom"))]
use std::time::Duration;

#[cfg(not(feature = "loom"))]
pub fn bench_dirty_write_arc_value(c: &mut Criterion) {
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
