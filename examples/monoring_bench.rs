use cachelog::{DirectRead, MonotonicRing, PAYLOAD_WORDS, Payload};
use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

#[inline]
fn make_payload(seed: u64) -> Payload {
    std::array::from_fn(|i| seed.wrapping_add(i as u64))
}

fn run_push_scale(readers: usize, secs: f64) -> f64 {
    let ring = Arc::new(MonotonicRing::<DirectRead>::new(1 << 12));
    let live_id = ring.push(make_payload(0));
    let stop = Arc::new(AtomicBool::new(false));
    let mut reader_threads = Vec::with_capacity(readers);

    for _ in 0..readers {
        let ring = Arc::clone(&ring);
        let stop = Arc::clone(&stop);
        reader_threads.push(thread::spawn(move || {
            let mut n = 0u64;
            while !stop.load(Ordering::Relaxed) {
                black_box(ring.read(live_id, |v| v[PAYLOAD_WORDS - 1]));
                n = n.wrapping_add(1);
            }
            n
        }));
    }

    let start = Instant::now();
    let dur = Duration::from_secs_f64(secs);
    let mut writes = 0u64;
    while start.elapsed() < dur {
        black_box(ring.push(make_payload(writes)));
        writes = writes.wrapping_add(1);
    }
    stop.store(true, Ordering::Relaxed);
    let _reads: u64 = reader_threads.into_iter().map(|t| t.join().unwrap()).sum();
    writes as f64 / secs / 1_000_000.0
}

fn run_writers(writers: usize, secs: f64) -> f64 {
    let ring = Arc::new(MonotonicRing::<DirectRead>::new(1 << 12));
    let stop = Arc::new(AtomicBool::new(false));
    let mut handles = Vec::with_capacity(writers);

    for tid in 0..writers {
        let ring = Arc::clone(&ring);
        let stop = Arc::clone(&stop);
        handles.push(thread::spawn(move || {
            let mut n = 0u64;
            let mut value = tid as u64;
            while !stop.load(Ordering::Relaxed) {
                black_box(ring.push(make_payload(value)));
                value = value.wrapping_add(writers as u64);
                n = n.wrapping_add(1);
            }
            n
        }));
    }

    thread::sleep(Duration::from_secs_f64(secs));
    stop.store(true, Ordering::Relaxed);
    let total: u64 = handles.into_iter().map(|t| t.join().unwrap()).sum();
    total as f64 / secs / 1_000_000.0
}

fn run_writers_with_4k_payload(writers: usize, secs: f64) -> f64 {
    let ring = Arc::new(MonotonicRing::<DirectRead>::new(1 << 12));
    let stop = Arc::new(AtomicBool::new(false));
    let mut handles = Vec::with_capacity(writers);

    for tid in 0..writers {
        let ring = Arc::clone(&ring);
        let stop = Arc::clone(&stop);
        handles.push(thread::spawn(move || {
            let mut n = 0u64;
            let mut value = tid as u64;
            while !stop.load(Ordering::Relaxed) {
                let mut stamp = 0u64;
                let payload = make_payload(value);
                for word in payload {
                    stamp ^= word.rotate_left(13);
                }
                black_box(ring.push(make_payload(stamp)));
                value = value.wrapping_add(writers as u64);
                n = n.wrapping_add(1);
            }
            n
        }));
    }

    thread::sleep(Duration::from_secs_f64(secs));
    stop.store(true, Ordering::Relaxed);
    let total: u64 = handles.into_iter().map(|t| t.join().unwrap()).sum();
    total as f64 / secs / 1_000_000.0
}

fn run_spin(threads: usize, secs: f64) -> f64 {
    let ring = Arc::new(MonotonicRing::<DirectRead>::new(1 << 12));
    let stop = Arc::new(AtomicBool::new(false));
    let mut handles = Vec::with_capacity(threads);

    for tid in 0..threads {
        let ring = Arc::clone(&ring);
        let stop = Arc::clone(&stop);
        handles.push(thread::spawn(move || {
            let mut n = 0u64;
            let mut value = tid as u64;
            while !stop.load(Ordering::Relaxed) {
                let id = ring.push(make_payload(value));
                black_box(id);
                black_box(ring.pop());
                value = value.wrapping_add(threads as u64);
                n = n.wrapping_add(1);
            }
            n
        }));
    }

    thread::sleep(Duration::from_secs_f64(secs));
    stop.store(true, Ordering::Relaxed);
    let total: u64 = handles.into_iter().map(|t| t.join().unwrap()).sum();
    total as f64 / secs / 1_000_000.0
}

fn run_spsc_push_pop(secs: f64) -> (f64, f64, f64) {
    let ring = Arc::new(MonotonicRing::<DirectRead>::new(1 << 12));
    let stop = Arc::new(AtomicBool::new(false));

    let rp = Arc::clone(&ring);
    let sp = Arc::clone(&stop);
    let producer = thread::spawn(move || {
        let mut n = 0u64;
        let mut v = 0u64;
        while !sp.load(Ordering::Relaxed) {
            black_box(rp.push(make_payload(v)));
            v = v.wrapping_add(1);
            n = n.wrapping_add(1);
        }
        n
    });

    let rc = Arc::clone(&ring);
    let sc = Arc::clone(&stop);
    let consumer = thread::spawn(move || {
        let mut ok = 0u64;
        let mut miss = 0u64;
        while !sc.load(Ordering::Relaxed) {
            if rc.pop_n(1) == 1 {
                ok = ok.wrapping_add(1);
            } else {
                miss = miss.wrapping_add(1);
                std::hint::spin_loop();
            }
        }
        (ok, miss)
    });

    thread::sleep(Duration::from_secs_f64(secs));
    stop.store(true, Ordering::Relaxed);

    let produced = producer.join().unwrap();
    let (consumed, misses) = consumer.join().unwrap();
    let pm = produced as f64 / secs / 1_000_000.0;
    let cm = consumed as f64 / secs / 1_000_000.0;
    let mm = misses as f64 / secs / 1_000_000.0;
    (pm, cm, mm)
}

fn main() {
    let secs = 2.0;

    println!("== push scale with readers ==");
    for &readers in &[0usize, 1, 2, 4, 8] {
        let mops = run_push_scale(readers, secs);
        println!("readers={readers} push_throughput={mops:.2} Mops/s");
    }

    println!("\n== multi-writer push scale ==");
    for &writers in &[1usize, 2, 4, 8] {
        let mops = run_writers(writers, secs);
        println!("writers={writers} push_throughput={mops:.2} Mops/s");
    }

    println!("\n== multi-writer push scale (4KiB payload prep+copy per push) ==");
    for &writers in &[1usize, 2, 4, 8] {
        let mops = run_writers_with_4k_payload(writers, secs);
        println!("writers={writers} push_throughput_4k={mops:.2} Mops/s");
    }

    println!("\n== push+pop spin scale ==");
    for &threads in &[1usize, 2, 4, 8] {
        let mops = run_spin(threads, secs);
        println!("threads={threads} push_pop_throughput={mops:.2} Mops/s");
    }

    println!("\n== spsc (1 producer, 1 consumer pop_n(1)) ==");
    let (pm, cm, mm) = run_spsc_push_pop(secs);
    println!("producer={pm:.2} Mops/s consumed={cm:.2} Mops/s empty_polls={mm:.2} Mops/s");
}
