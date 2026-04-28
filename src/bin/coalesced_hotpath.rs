use std::env;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use ahash::RandomState as AHashState;
use cachelog::{CacheLogConfig, CacheLogMap, DirtyWriteMode, low_level::DirtyRecord};
use std::sync::Arc;

// One table drives usage text and dispatch so experiment names do not drift.
macro_rules! hotpath_mode_table {
    ($callback:ident, $iters:ident, $cap_override:ident, $flush_every:ident $(, $prefix:expr)? $(,)?) => {
        $callback! {
            $($prefix,)?
            "serial" => run_serial($iters, false, $cap_override),
            "serial-quiet" => run_serial($iters, true, $cap_override),
            "batch-unique" => run_batch_unique($iters, false, $cap_override, $flush_every),
            "batch-unique-quiet" => run_batch_unique($iters, true, $cap_override, $flush_every),
            "strict-batch-repeated-quiet" => {
                run_batch_repeated($iters, true, $cap_override, DirtyWriteMode::StrictLog)
            },
            "coalesced-batch-repeated-quiet" => {
                run_batch_repeated($iters, true, $cap_override, DirtyWriteMode::CoalescedMap)
            },
            "flush-batch-unique-quiet" => run_flush_batch_unique($iters, $cap_override),
            "flush-batch-build-multi-quiet" => run_flush_batch_build_multi($iters, $cap_override),
            "mark-flushed-unique-quiet" => run_mark_flushed_unique($iters, $cap_override),
            "mark-flushed-multi-quiet" => run_mark_flushed_multi($iters, $cap_override),
            "materialize-dirty-arcs-multi-quiet" => {
                run_materialize_dirty_arcs_multi($iters, $cap_override)
            },
            "materialize-dirty-arcs-keyclone-multi-quiet" => {
                run_materialize_dirty_arcs_keyclone_multi($iters, $cap_override)
            },
            "clone-keys-multi-quiet" => run_clone_keys_multi($iters, $cap_override),
        }
    };
}

macro_rules! hotpath_mode_names {
    ($($name:literal => $run:expr),+ $(,)?) => {
        &[$($name),+]
    };
}

macro_rules! dispatch_hotpath_mode {
    ($mode:expr, $($name:literal => $run:expr),+ $(,)?) => {
        match $mode {
            $($name => $run,)+
            _ => usage(),
        }
    };
}

const HOTPATH_MODE_NAMES: &[&str] =
    hotpath_mode_table!(hotpath_mode_names, iters, cap_override, flush_every);

const HOTPATH_EXAMPLES: &[&str] = &[
    "serial 1000000",
    "batch-unique 10000",
    "coalesced-batch-repeated-quiet 10000",
    "clone-keys-multi-quiet 10000",
    "scc-upsert-overwrite-prepared-quiet 10000",
    "scc-upsert-overwrite-u64-u64-prepared-quiet 10000",
    "scc-upsert-overwrite-u64-box-prepared-quiet 10000",
    "scc-upsert-overwrite-array8-u64-prepared-quiet 10000",
    "scc-upsert-overwrite-boxed-array8-u64-prepared-quiet 10000",
    "scc-upsert-overwrite-arc-slice-u64-prepared-quiet 10000",
    "batch-unique 10000 2001",
    "batch-unique 10000 2001 2",
];

fn usage() -> ! {
    eprintln!(
        "usage: coalesced_hotpath <{}> <iters> [cap] [flush_every]",
        HOTPATH_MODE_NAMES.join("|")
    );
    eprintln!("         examples:");
    for example in HOTPATH_EXAMPLES {
        eprintln!("         coalesced_hotpath {example}");
    }
    std::process::exit(2);
}

fn parse_iters(arg: Option<String>) -> u64 {
    match arg.and_then(|s| s.parse::<u64>().ok()) {
        Some(v) if v > 0 => v,
        _ => usage(),
    }
}

fn parse_cap(arg: Option<String>) -> Option<usize> {
    arg.and_then(|s| s.parse::<usize>().ok()).filter(|v| *v > 0)
}

fn parse_flush_every(arg: Option<String>) -> Option<u64> {
    arg.and_then(|s| s.parse::<u64>().ok()).filter(|v| *v > 0)
}

fn maybe_profile_delay() {
    let Some(ms) = env::var("COALESCED_HOTPATH_DELAY_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|v| *v > 0)
    else {
        return;
    };
    thread::sleep(Duration::from_millis(ms));
}

fn visible_noise_entries() -> usize {
    env::var("COALESCED_VISIBLE_NOISE")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0)
}

fn build_map(cap: usize) -> CacheLogMap<Vec<u8>, u64, AHashState> {
    let cfg = CacheLogConfig::new(cap, cap, 16).with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
    CacheLogMap::with_hasher(cfg, AHashState::new())
}

fn build_keys(count: usize) -> Vec<Vec<u8>> {
    (0..count)
        .map(|seq| format!("uniq:{seq:016}").into_bytes())
        .collect()
}

fn build_hot_keys(count: usize) -> Vec<Vec<u8>> {
    (0..count)
        .map(|seq| format!("hot:{seq:04}").into_bytes())
        .collect()
}

fn run_serial(iters: u64, quiet: bool, cap_override: Option<usize>) {
    let cap = cap_override.unwrap_or(iters as usize + 16);
    let map = build_map(cap);
    let keys = build_keys(iters as usize);
    maybe_profile_delay();
    let start = Instant::now();
    for (seq, key) in keys.into_iter().enumerate() {
        let id = map.low_level().insert_dirty(key, seq as u64);
        std::hint::black_box(id);
    }
    let elapsed = start.elapsed();
    std::mem::forget(map);
    if !quiet {
        let secs = elapsed.as_secs_f64();
        let rate = iters as f64 / secs;
        println!(
            "mode=serial iters={} seconds={:.6} rate={:.2}",
            iters, secs, rate
        );
    } else {
        std::hint::black_box(elapsed);
    }
}

fn run_batch_unique(
    iters: u64,
    quiet: bool,
    cap_override: Option<usize>,
    flush_every: Option<u64>,
) {
    let writes = iters as usize * 1_000;
    let cap = cap_override.unwrap_or(writes + 16);
    let map = build_map(cap);
    maybe_profile_delay();
    let start = Instant::now();
    let mut seq = 0_u64;
    for batch_idx in 0..iters {
        let mut batch = Vec::with_capacity(1_000);
        for _ in 0..1_000 {
            let value = seq;
            batch.push((format!("uniq:{value:016}").into_bytes(), value));
            seq = seq.wrapping_add(1);
        }
        let written = map.low_level().insert_dirty_batch_without_ids(batch);
        std::hint::black_box(written);
        let should_flush = match flush_every {
            Some(every) => (batch_idx + 1) % every == 0,
            None => false,
        };
        if should_flush {
            let flushed = map
                .flush_now(1_000_000, |_| Ok::<(), ()>(()))
                .expect("no-op flush should succeed");
            std::hint::black_box(flushed);
        }
    }
    let elapsed = start.elapsed();
    std::mem::forget(map);
    if !quiet {
        let secs = elapsed.as_secs_f64();
        let writes_f = (iters * 1_000) as f64;
        let rate = writes_f / secs;
        println!(
            "mode=batch-unique batches={} writes={} seconds={:.6} rate={:.2}",
            iters,
            iters * 1_000,
            secs,
            rate
        );
    } else {
        std::hint::black_box(elapsed);
    }
}

fn run_batch_repeated(iters: u64, quiet: bool, cap_override: Option<usize>, mode: DirtyWriteMode) {
    let writes = iters as usize * 1_000;
    let cap = cap_override.unwrap_or(writes + 16);
    let cfg = CacheLogConfig::new(cap, cap, 16).with_dirty_write_mode(mode);
    let map = CacheLogMap::<Vec<u8>, u64, AHashState>::with_hasher(cfg, AHashState::new());
    let keys = build_hot_keys(64);
    maybe_profile_delay();
    let start = Instant::now();
    let mut seq = 0_u64;
    for _ in 0..iters {
        let mut batch = Vec::with_capacity(1_000);
        for i in 0..1_000 {
            let value = seq;
            batch.push((keys[i % keys.len()].clone(), value));
            seq = seq.wrapping_add(1);
        }
        let written = map.low_level().insert_dirty_batch_without_ids(batch);
        std::hint::black_box(written);
    }
    let elapsed = start.elapsed();
    std::mem::forget(map);
    if !quiet {
        let secs = elapsed.as_secs_f64();
        let writes_f = (iters * 1_000) as f64;
        let rate = writes_f / secs;
        println!(
            "mode=batch-repeated dirty_write_mode={mode:?} batches={} writes={} seconds={:.6} rate={:.2}",
            iters,
            iters * 1_000,
            secs,
            rate
        );
    } else {
        std::hint::black_box(elapsed);
    }
}

fn run_flush_batch_unique(iters: u64, cap_override: Option<usize>) {
    let cap = cap_override.unwrap_or(2_001);
    let map = build_map(cap);
    let mut seq = 0_u64;
    for _ in 0..iters {
        let mut batch = Vec::with_capacity(1_000);
        for _ in 0..1_000 {
            let value = seq;
            batch.push((format!("uniq:{value:016}").into_bytes(), value));
            seq = seq.wrapping_add(1);
        }
        let written = map.low_level().insert_dirty_batch_without_ids(batch);
        std::hint::black_box(written);
    }
    maybe_profile_delay();
    let start = Instant::now();
    let flushed = map
        .flush_now(1_000_000, |_| Ok::<(), ()>(()))
        .expect("no-op flush should succeed");
    let elapsed = start.elapsed();
    std::mem::forget(map);
    std::hint::black_box(flushed);
    std::hint::black_box(elapsed);
}

fn run_mark_flushed_unique(iters: u64, cap_override: Option<usize>) {
    let cap = cap_override.unwrap_or(2_001);
    let map = build_map(cap);
    let mut seq = 0_u64;
    for _ in 0..iters {
        let mut batch = Vec::with_capacity(1_000);
        for _ in 0..1_000 {
            let value = seq;
            batch.push((format!("uniq:{value:016}").into_bytes(), value));
            seq = seq.wrapping_add(1);
        }
        let written = map.low_level().insert_dirty_batch_without_ids(batch);
        std::hint::black_box(written);
    }
    let batch = map.low_level().flush_batch(1_000_000);
    let start = Instant::now();
    let flushed = map.low_level().mark_flushed(&batch);
    let elapsed = start.elapsed();
    std::mem::forget(map);
    std::hint::black_box(flushed);
    std::hint::black_box(elapsed);
}

fn fill_unique_entries(
    map: &CacheLogMap<Vec<u8>, u64, AHashState>,
    start_seq: &mut u64,
    count: usize,
) {
    let mut batch = Vec::with_capacity(count);
    for _ in 0..count {
        let value = *start_seq;
        batch.push((format!("uniq:{value:016}").into_bytes(), value));
        *start_seq = start_seq.wrapping_add(1);
    }
    let written = map.low_level().insert_dirty_batch_without_ids(batch);
    std::hint::black_box(written);
}

fn fill_clean_noise(
    map: &CacheLogMap<Vec<u8>, u64, AHashState>,
    start_seq: &mut u64,
    count: usize,
) {
    for _ in 0..count {
        let value = *start_seq;
        let key = format!("clean-noise:{value:016}").into_bytes();
        let inserted = map.insert_clean_if_absent(key, value);
        std::hint::black_box(inserted);
        *start_seq = start_seq.wrapping_add(1);
    }
}

fn run_flush_batch_build_multi(iters: u64, cap_override: Option<usize>) {
    let cap = cap_override.unwrap_or(2_001);
    let mut maps = Vec::with_capacity(iters as usize);
    let mut seq = 0_u64;
    let noise = visible_noise_entries();
    for _ in 0..iters {
        let map = build_map(cap);
        fill_unique_entries(&map, &mut seq, 1_000);
        if noise > 0 {
            fill_clean_noise(&map, &mut seq, noise);
        }
        maps.push(map);
    }
    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for map in &maps {
        let batch = map.low_level().flush_batch(1_000_000);
        total = total.wrapping_add(batch.len());
        std::hint::black_box(batch);
    }
    let elapsed = start.elapsed();
    std::mem::forget(maps);
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
}

fn run_mark_flushed_multi(iters: u64, cap_override: Option<usize>) {
    let cap = cap_override.unwrap_or(2_001);
    let mut pending = Vec::with_capacity(iters as usize);
    let mut seq = 0_u64;
    let noise = visible_noise_entries();
    for _ in 0..iters {
        let map = build_map(cap);
        fill_unique_entries(&map, &mut seq, 1_000);
        let batch = map.low_level().flush_batch(1_000_000);
        if noise > 0 {
            fill_clean_noise(&map, &mut seq, noise);
        }
        pending.push((map, batch));
    }
    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for (map, batch) in &pending {
        let flushed = map.low_level().mark_flushed(batch);
        total = total.wrapping_add(flushed);
        std::hint::black_box(flushed);
    }
    let elapsed = start.elapsed();
    std::mem::forget(pending);
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
}

fn run_materialize_dirty_arcs_multi(iters: u64, cap_override: Option<usize>) {
    let count = cap_override.unwrap_or(1_000);
    let mut seq = 0_u64;
    let mut inputs = Vec::with_capacity(iters as usize);
    for _ in 0..iters {
        let mut batch = Vec::with_capacity(count);
        for _ in 0..count {
            let value = seq;
            batch.push((format!("uniq:{value:016}").into_bytes(), value));
            seq = seq.wrapping_add(1);
        }
        inputs.push(batch);
    }
    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for batch in inputs {
        let entries = batch
            .into_iter()
            .enumerate()
            .map(|(offset, (key, value))| {
                Arc::new(DirtyRecord {
                    id: offset as u64,
                    key,
                    value,
                })
            })
            .collect::<Vec<_>>();
        total = total.wrapping_add(entries.len());
        std::hint::black_box(entries);
    }
    let elapsed = start.elapsed();
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
}

fn run_materialize_dirty_arcs_keyclone_multi(iters: u64, cap_override: Option<usize>) {
    let count = cap_override.unwrap_or(1_000);
    let mut seq = 0_u64;
    let mut inputs = Vec::with_capacity(iters as usize);
    for _ in 0..iters {
        let mut batch = Vec::with_capacity(count);
        for _ in 0..count {
            let value = seq;
            batch.push((format!("uniq:{value:016}").into_bytes(), value));
            seq = seq.wrapping_add(1);
        }
        inputs.push(batch);
    }
    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for batch in inputs {
        let entries = batch
            .into_iter()
            .enumerate()
            .map(|(offset, key_value)| {
                let (key, value) = key_value;
                Arc::new(DirtyRecord {
                    id: offset as u64,
                    key: key.clone(),
                    value,
                })
            })
            .collect::<Vec<_>>();
        total = total.wrapping_add(entries.len());
        std::hint::black_box(entries);
    }
    let elapsed = start.elapsed();
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
}

fn run_clone_keys_multi(iters: u64, cap_override: Option<usize>) {
    let count = cap_override.unwrap_or(1_000);
    let mut seq = 0_u64;
    let mut inputs = Vec::with_capacity(iters as usize);
    for _ in 0..iters {
        let mut batch = Vec::with_capacity(count);
        for _ in 0..count {
            let value = seq;
            batch.push(format!("uniq:{value:016}").into_bytes());
            seq = seq.wrapping_add(1);
        }
        inputs.push(batch);
    }
    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for batch in &inputs {
        let clones = batch.to_vec();
        total = total.wrapping_add(clones.len());
        std::hint::black_box(clones);
    }
    let elapsed = start.elapsed();
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
}

fn main() {
    let mut args = env::args().skip(1);
    let mode = args.next().unwrap_or_else(|| usage());
    let iters = parse_iters(args.next());
    let cap_override = parse_cap(args.next());
    let flush_every = parse_flush_every(args.next());
    hotpath_mode_table!(
        dispatch_hotpath_mode,
        iters,
        cap_override,
        flush_every,
        mode.as_str(),
    );
}
