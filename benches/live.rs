#[path = "live/bytes.rs"]
mod bytes;
#[path = "live/mixed.rs"]
mod mixed;
#[path = "live/prefix.rs"]
mod prefix;
#[path = "live/read.rs"]
mod read;
#[path = "live/stale.rs"]
mod stale;
#[path = "live/write.rs"]
mod write;

#[cfg(not(feature = "loom"))]
criterion::criterion_group!(
    benches,
    write::bench_dirty_write,
    write::bench_dirty_write_coalesced_focus,
    read::bench_dirty_read,
    bytes::bench_dirty_write_arc_value,
    write::bench_dirty_write_batch_modes,
    read::bench_borrowed_lookup,
    read::bench_dirty_read_under_write,
    mixed::bench_mixed_rw,
    prefix::bench_prefix_list_with_advance,
    stale::bench_stale_cycle,
    stale::bench_stale_breakdown
);

#[cfg(feature = "loom")]
criterion::criterion_group!(
    benches,
    write::bench_dirty_write,
    read::bench_dirty_read,
    write::bench_dirty_write_batch_modes,
    read::bench_borrowed_lookup,
    read::bench_dirty_read_under_write,
    mixed::bench_mixed_rw,
    prefix::bench_prefix_list_with_advance,
    stale::bench_stale_cycle,
    stale::bench_stale_breakdown
);

criterion::criterion_main!(benches);
