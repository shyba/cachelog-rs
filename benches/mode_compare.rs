use std::hint::{black_box, spin_loop};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use cachelog::{CacheLogConfig, CacheLogMap, IdRingMap};
use criterion::{Criterion, criterion_group, criterion_main};

struct Stats {
    writes: AtomicU64,
    reads: AtomicU64,
    read_hits: AtomicU64,
    flushed: AtomicU64,
}

impl Stats {
    fn new() -> Self {
        Self {
            writes: AtomicU64::new(0),
            reads: AtomicU64::new(0),
            read_hits: AtomicU64::new(0),
            flushed: AtomicU64::new(0),
        }
    }
}

#[derive(Clone, Copy)]
struct XorShift64(u64);

impl XorShift64 {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

#[derive(Clone, Copy)]
struct Scenario {
    writers: usize,
    readers: usize,
    keyspace: u64,
    flush_batch: usize,
    secs: f64,
}

impl Default for Scenario {
    fn default() -> Self {
        Self {
            writers: 8,
            readers: 4,
            keyspace: 200_000,
            flush_batch: 256,
            secs: 2.0,
        }
    }
}

fn run_current_mode(s: Scenario) -> (f64, f64, f64, f64) {
    let map = Arc::new(CacheLogMap::<u64, u64>::new(CacheLogConfig::new(
        1 << 20,
        1 << 20,
        1 << 10,
    )));
    let stop = Arc::new(AtomicBool::new(false));
    let stats = Arc::new(Stats::new());

    let mut handles = Vec::new();

    for tid in 0..s.writers {
        let map = Arc::clone(&map);
        let stop = Arc::clone(&stop);
        let stats = Arc::clone(&stats);
        handles.push(thread::spawn(move || {
            let mut rng = XorShift64::new(0xA000_0000_0000_0000 | tid as u64);
            while !stop.load(Ordering::Relaxed) {
                let key = rng.next_u64() % s.keyspace;
                let value = rng.next_u64();
                black_box(map.insert_dirty(key, value));
                stats.writes.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    for tid in 0..s.readers {
        let map = Arc::clone(&map);
        let stop = Arc::clone(&stop);
        let stats = Arc::clone(&stats);
        handles.push(thread::spawn(move || {
            let mut rng = XorShift64::new(0xB000_0000_0000_0000 | tid as u64);
            while !stop.load(Ordering::Relaxed) {
                let key = rng.next_u64() % s.keyspace;
                if let Some(value) = map.read(&key, |_, v, _, _| *v) {
                    black_box(value);
                    stats.read_hits.fetch_add(1, Ordering::Relaxed);
                }
                stats.reads.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    {
        let map = Arc::clone(&map);
        let stop = Arc::clone(&stop);
        let stats = Arc::clone(&stats);
        handles.push(thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let batch = map.flush_batch(s.flush_batch);
                if !batch.is_empty() {
                    let n = map.mark_flushed(&batch) as u64;
                    stats.flushed.fetch_add(n, Ordering::Relaxed);
                } else {
                    spin_loop();
                }
            }
        }));
    }

    thread::sleep(Duration::from_secs_f64(s.secs));
    stop.store(true, Ordering::Relaxed);
    for h in handles {
        let _ = h.join();
    }

    let t = s.secs;
    (
        stats.writes.load(Ordering::Relaxed) as f64 / t / 1e6,
        stats.reads.load(Ordering::Relaxed) as f64 / t / 1e6,
        stats.read_hits.load(Ordering::Relaxed) as f64 / t / 1e6,
        stats.flushed.load(Ordering::Relaxed) as f64 / t / 1e6,
    )
}

fn run_id_ring_mode(s: Scenario) -> (f64, f64, f64, f64) {
    let map = Arc::new(IdRingMap::new(1 << 20, 1 << 20));
    let stop = Arc::new(AtomicBool::new(false));
    let stats = Arc::new(Stats::new());

    let mut handles = Vec::new();

    for tid in 0..s.writers {
        let map = Arc::clone(&map);
        let stop = Arc::clone(&stop);
        let stats = Arc::clone(&stats);
        handles.push(thread::spawn(move || {
            let mut rng = XorShift64::new(0xC000_0000_0000_0000 | tid as u64);
            while !stop.load(Ordering::Relaxed) {
                let key = rng.next_u64() % s.keyspace;
                let value = rng.next_u64();
                black_box(map.upsert(key, value));
                stats.writes.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    for tid in 0..s.readers {
        let map = Arc::clone(&map);
        let stop = Arc::clone(&stop);
        let stats = Arc::clone(&stats);
        handles.push(thread::spawn(move || {
            let mut rng = XorShift64::new(0xD000_0000_0000_0000 | tid as u64);
            while !stop.load(Ordering::Relaxed) {
                let key = rng.next_u64() % s.keyspace;
                if let Some(value) = map.read(key) {
                    black_box(value);
                    stats.read_hits.fetch_add(1, Ordering::Relaxed);
                }
                stats.reads.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    {
        let map = Arc::clone(&map);
        let stop = Arc::clone(&stop);
        let stats = Arc::clone(&stats);
        handles.push(thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let n = map.flush_advance(s.flush_batch as u64);
                if n > 0 {
                    stats.flushed.fetch_add(n, Ordering::Relaxed);
                } else {
                    spin_loop();
                }
            }
        }));
    }

    thread::sleep(Duration::from_secs_f64(s.secs));
    stop.store(true, Ordering::Relaxed);
    for h in handles {
        let _ = h.join();
    }

    let t = s.secs;
    (
        stats.writes.load(Ordering::Relaxed) as f64 / t / 1e6,
        stats.reads.load(Ordering::Relaxed) as f64 / t / 1e6,
        stats.read_hits.load(Ordering::Relaxed) as f64 / t / 1e6,
        stats.flushed.load(Ordering::Relaxed) as f64 / t / 1e6,
    )
}

fn bench_modes(c: &mut Criterion) {
    let scenario = Scenario::default();
    let mut group = c.benchmark_group("mode_compare");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(6));
    group.warm_up_time(Duration::from_secs(1));

    group.bench_function("current", |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                let r = run_current_mode(scenario);
                black_box(r);
            }
            start.elapsed()
        });
    });

    group.bench_function("id_ring", |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                let r = run_id_ring_mode(scenario);
                black_box(r);
            }
            start.elapsed()
        });
    });
    group.finish();
}

criterion_group!(benches, bench_modes);
criterion_main!(benches);
