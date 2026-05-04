use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use cachelog::{BytePrefixMap, CacheLogConfig};
use criterion::{BenchmarkId, Criterion, Throughput};

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

pub fn bench_prefix_list_with_advance(c: &mut Criterion) {
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
        b.iter(|| black_box(map.list_prefix(b"ab:", |_, value, _| *value, 5_000).len()));
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
            b.iter(|| black_box(map.list_prefix(b"ab:", |_, value, _| *value, 5_000).len()));
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
                black_box(map.list_prefix(b"ab:", |_, value, _| *value, 5_000).len());
                black_box(map.read(b"ab:00000000", |_, value, _| *value));
            });
            stop.store(true, Ordering::Release);
            writer.join().unwrap();
            advancer.join().unwrap();
            reader.join().unwrap();
        },
    );
    group.finish();
}
