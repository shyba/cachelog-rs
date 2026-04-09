use std::hint::black_box;
use std::time::Duration;

use cachelog::{CheckedRead, DirectRead, MonotonicRing, PAYLOAD_WORDS, Payload};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

#[inline]
fn make_payload(seed: u64) -> Payload {
    std::array::from_fn(|i| seed.wrapping_add(i as u64))
}

fn bench_serial(c: &mut Criterion) {
    let mut group = c.benchmark_group("monoring_serial");
    group.measurement_time(Duration::from_secs(3));
    group.throughput(Throughput::Elements(1));

    group.bench_function(BenchmarkId::new("push", "direct"), |b| {
        let direct = MonotonicRing::<DirectRead>::new(1 << 12);
        let mut v = 0_u64;
        b.iter(|| {
            let id = direct.push(make_payload(v));
            black_box(id);
            v = v.wrapping_add(1);
        });
    });

    group.bench_function(BenchmarkId::new("read_hit", "direct"), |b| {
        let direct = MonotonicRing::<DirectRead>::new(1 << 12);
        let direct_id = direct.push(make_payload(77));
        b.iter(|| {
            let got = direct.read(direct_id, |v| v[PAYLOAD_WORDS - 1]).unwrap();
            black_box(got);
        });
    });

    group.bench_function(BenchmarkId::new("read_hit", "checked"), |b| {
        let checked = MonotonicRing::<CheckedRead>::new(1 << 12);
        let checked_id = checked.push(make_payload(77));
        b.iter(|| {
            let got = checked.read(checked_id, |v| v[PAYLOAD_WORDS - 1]).unwrap();
            black_box(got);
        });
    });

    group.bench_function(BenchmarkId::new("push_pop", "direct"), |b| {
        let ring = MonotonicRing::<DirectRead>::new(1 << 12);
        let mut v = 0_u64;
        b.iter(|| {
            let _ = ring.push(make_payload(v));
            let popped = ring.pop();
            black_box(popped);
            v = v.wrapping_add(1);
        });
    });

    group.finish();
}

criterion_group!(benches, bench_serial);
criterion_main!(benches);
