use std::env;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use ahash::{AHashMap, RandomState as AHashState};
use bytes::Bytes;
use cachelog::{CacheLogConfig, CacheLogMap, DirtyWriteMode, low_level::DirtyRecord};
use scc::HashMap as ConcurrentHashMap;
use std::sync::Arc;

type PreparedCompactedBatch = Vec<(Vec<u8>, Box<(u64, u64)>)>;

// One table drives usage text and dispatch so experiment names do not drift.
macro_rules! hotpath_mode_table {
    ($callback:ident, $iters:ident, $cap_override:ident, $flush_every:ident $(, $prefix:expr)? $(,)?) => {
        $callback! {
            $($prefix,)?
            "serial" => run_serial($iters, false, $cap_override),
            "serial-quiet" => run_serial($iters, true, $cap_override),
            "serial-prebuilt-quiet" => run_serial_prebuilt($iters, $cap_override),
            "batch-unique" => run_batch_unique($iters, false, $cap_override, $flush_every),
            "batch-unique-quiet" => run_batch_unique($iters, true, $cap_override, $flush_every),
            "batch-unique-prebuilt-quiet" => run_batch_unique_prebuilt($iters, $cap_override),
            "strict-batch-repeated-quiet" => {
                run_batch_repeated($iters, true, $cap_override, DirtyWriteMode::StrictLog)
            },
            "coalesced-batch-repeated-quiet" => {
                run_batch_repeated($iters, true, $cap_override, DirtyWriteMode::CoalescedMap)
            },
            "coalesced-batch-repeated-prebuilt-quiet" => {
                run_batch_repeated_prebuilt($iters, $cap_override, false)
            },
            "coalesced-batch-repeated-compacted-prebuilt-quiet" => {
                run_batch_repeated_prebuilt($iters, $cap_override, true)
            },
            "compact-repeated-prebuilt-quiet" => run_compact_repeated_prebuilt($iters, $cap_override),
            "compact-repeated-prebuilt-reuse-quiet" => {
                run_compact_repeated_prebuilt_reuse($iters, $cap_override)
            },
            "compact-reuse-plus-update-vec-box-prebuilt-quiet" => {
                run_compact_reuse_plus_update_vec_box_prebuilt($iters, $cap_override)
            },
            "consume-repeated-compacted-prebuilt-quiet" => {
                run_consume_repeated_compacted_prebuilt($iters, $cap_override)
            },
            "clone-keys-repeated-compacted-prebuilt-quiet" => {
                run_clone_keys_repeated_compacted_prebuilt($iters, $cap_override)
            },
            "hash-keys-repeated-compacted-prebuilt-quiet" => {
                run_hash_keys_repeated_compacted_prebuilt($iters, $cap_override)
            },
            "box-records-repeated-compacted-prebuilt-quiet" => {
                run_box_records_repeated_compacted_prebuilt($iters, $cap_override)
            },
            "clone-keys-repeated-compacted-borrowed-quiet" => {
                run_clone_keys_repeated_compacted_borrowed($iters, $cap_override)
            },
            "hash-keys-repeated-compacted-borrowed-quiet" => {
                run_hash_keys_repeated_compacted_borrowed($iters, $cap_override)
            },
            "box-records-repeated-compacted-borrowed-quiet" => {
                run_box_records_repeated_compacted_borrowed($iters, $cap_override)
            },
            "clone-and-box-repeated-compacted-borrowed-quiet" => {
                run_clone_and_box_repeated_compacted_borrowed($iters, $cap_override)
            },
            "clone-and-box-repeated-compacted-borrowed-leak-quiet" => {
                run_clone_and_box_repeated_compacted_borrowed_leak($iters, $cap_override)
            },
            "convert-keys-to-vec-repeated-compacted-borrowed-quiet" => {
                run_convert_keys_to_vec_repeated_compacted_borrowed($iters, $cap_override)
            },
            "convert-keys-to-arc-slice-repeated-compacted-borrowed-quiet" => {
                run_convert_keys_to_arc_slice_repeated_compacted_borrowed($iters, $cap_override)
            },
            "convert-keys-to-bytes-repeated-compacted-borrowed-quiet" => {
                run_convert_keys_to_bytes_repeated_compacted_borrowed($iters, $cap_override)
            },
            "scc-upsert-overwrite-vec-u64-from-borrowed-quiet" => {
                run_scc_upsert_overwrite_vec_u64_from_borrowed($iters, $cap_override)
            },
            "scc-update-or-insert-vec-u64-from-borrowed-quiet" => {
                run_scc_update_or_insert_vec_u64_from_borrowed($iters, $cap_override)
            },
            "scc-upsert-overwrite-vec-box-from-borrowed-quiet" => {
                run_scc_upsert_overwrite_vec_box_from_borrowed($iters, $cap_override)
            },
            "scc-update-or-insert-vec-box-from-borrowed-quiet" => {
                run_scc_update_or_insert_vec_box_from_borrowed($iters, $cap_override)
            },
            "scc-upsert-overwrite-arc-slice-u64-from-borrowed-quiet" => {
                run_scc_upsert_overwrite_arc_slice_u64_from_borrowed($iters, $cap_override)
            },
            "scc-upsert-overwrite-bytes-crate-u64-from-borrowed-quiet" => {
                run_scc_upsert_overwrite_bytes_crate_u64_from_borrowed($iters, $cap_override)
            },
            "scc-upsert-overwrite-prepared-quiet" => {
                run_scc_upsert_overwrite_prepared($iters, $cap_override, false)
            },
            "scc-upsert-overwrite-prepared-forget-old-quiet" => {
                run_scc_upsert_overwrite_prepared($iters, $cap_override, true)
            },
            "scc-upsert-overwrite-bytes-u64-prepared-quiet" => {
                run_scc_upsert_overwrite_bytes_u64_prepared($iters, $cap_override)
            },
            "scc-upsert-overwrite-arc-slice-u64-prepared-quiet" => {
                run_scc_upsert_overwrite_arc_slice_u64_prepared($iters, $cap_override)
            },
            "scc-upsert-overwrite-bytes-crate-u64-prepared-quiet" => {
                run_scc_upsert_overwrite_bytes_crate_u64_prepared($iters, $cap_override)
            },
            "scc-upsert-overwrite-u64-u64-prepared-quiet" => {
                run_scc_upsert_overwrite_u64_u64_prepared($iters, $cap_override)
            },
            "scc-upsert-overwrite-u64-box-prepared-quiet" => {
                run_scc_upsert_overwrite_u64_box_prepared($iters, $cap_override)
            },
            "scc-upsert-overwrite-array8-u64-prepared-quiet" => {
                run_scc_upsert_overwrite_array8_u64_prepared($iters, $cap_override)
            },
            "scc-upsert-overwrite-boxed-array8-u64-prepared-quiet" => {
                run_scc_upsert_overwrite_boxed_array8_u64_prepared($iters, $cap_override)
            },
            "scc-upsert-overwrite-array16-u64-prepared-quiet" => {
                run_scc_upsert_overwrite_array16_u64_prepared($iters, $cap_override)
            },
            "flush-batch-unique-quiet" => run_flush_batch_unique($iters, $cap_override),
            "flush-batch-build-unique-quiet" => run_flush_batch_build_unique($iters, $cap_override),
            "mark-flushed-unique-quiet" => run_mark_flushed_unique($iters, $cap_override),
            "flush-batch-build-multi-quiet" => run_flush_batch_build_multi($iters, $cap_override),
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
    "compact-repeated-prebuilt-quiet 10000",
    "consume-repeated-compacted-prebuilt-quiet 10000",
    "clone-keys-repeated-compacted-prebuilt-quiet 10000",
    "clone-and-box-repeated-compacted-borrowed-quiet 10000",
    "clone-and-box-repeated-compacted-borrowed-leak-quiet 10000",
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

fn build_repeated_batches(
    iters: u64,
    hot_keys: usize,
    batch_len: usize,
) -> Vec<Vec<(Vec<u8>, u64)>> {
    let keys = build_hot_keys(hot_keys);
    let mut batches = Vec::with_capacity(iters as usize);
    let mut seq = 0_u64;
    for _ in 0..iters {
        let mut batch = Vec::with_capacity(batch_len);
        for i in 0..batch_len {
            let value = seq;
            batch.push((keys[i % keys.len()].clone(), value));
            seq = seq.wrapping_add(1);
        }
        batches.push(batch);
    }
    batches
}

fn compact_last_write_wins_owned(entries: Vec<(Vec<u8>, u64)>) -> Vec<(Vec<u8>, u64)> {
    let mut latest_slot = AHashMap::with_capacity(entries.len());
    let mut compacted = Vec::with_capacity(entries.len());

    for (key, value) in entries {
        if let Some(&slot) = latest_slot.get(&key) {
            if let Some((_, current_value)) = compacted.get_mut(slot) {
                *current_value = value;
            }
        } else {
            latest_slot.insert(key.clone(), compacted.len());
            compacted.push((key, value));
        }
    }

    compacted
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

fn run_serial_prebuilt(iters: u64, cap_override: Option<usize>) {
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
    std::hint::black_box(elapsed);
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
        if let Some(every) = flush_every
            && (batch_idx + 1) % every == 0
        {
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

fn build_unique_batches(iters: u64, batch_len: usize) -> Vec<Vec<(Vec<u8>, u64)>> {
    let mut batches = Vec::with_capacity(iters as usize);
    let mut seq = 0_u64;
    for _ in 0..iters {
        let mut batch = Vec::with_capacity(batch_len);
        for _ in 0..batch_len {
            let value = seq;
            batch.push((format!("uniq:{value:016}").into_bytes(), value));
            seq = seq.wrapping_add(1);
        }
        batches.push(batch);
    }
    batches
}

fn run_batch_unique_prebuilt(iters: u64, cap_override: Option<usize>) {
    let writes = iters as usize * 1_000;
    let cap = cap_override.unwrap_or(writes + 16);
    let map = build_map(cap);
    let batches = build_unique_batches(iters, 1_000);
    maybe_profile_delay();
    let start = Instant::now();
    for batch in batches {
        let written = map.low_level().insert_dirty_batch_without_ids(batch);
        std::hint::black_box(written);
    }
    let elapsed = start.elapsed();
    std::mem::forget(map);
    std::hint::black_box(elapsed);
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

fn run_batch_repeated_prebuilt(iters: u64, cap_override: Option<usize>, compact_first: bool) {
    let writes = iters as usize * 1_000;
    let cap = cap_override.unwrap_or(writes + 16);
    let cfg = CacheLogConfig::new(cap, cap, 16).with_dirty_write_mode(DirtyWriteMode::CoalescedMap);
    let map = CacheLogMap::<Vec<u8>, u64, AHashState>::with_hasher(cfg, AHashState::new());
    let batches = if compact_first {
        build_repeated_batches(iters, 64, 1_000)
            .into_iter()
            .map(compact_last_write_wins_owned)
            .collect::<Vec<_>>()
    } else {
        build_repeated_batches(iters, 64, 1_000)
    };
    maybe_profile_delay();
    let start = Instant::now();
    for batch in batches {
        let written = map.low_level().insert_dirty_batch_without_ids(batch);
        std::hint::black_box(written);
    }
    let elapsed = start.elapsed();
    std::mem::forget(map);
    std::hint::black_box(elapsed);
}

fn run_compact_repeated_prebuilt(iters: u64, cap_override: Option<usize>) {
    let batch_len = cap_override.unwrap_or(1_000);
    let batches = build_repeated_batches(iters, 64, batch_len);
    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for batch in batches {
        let compacted = compact_last_write_wins_owned(batch);
        total = total.wrapping_add(compacted.len());
        std::hint::black_box(compacted);
    }
    let elapsed = start.elapsed();
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
}

fn run_compact_repeated_prebuilt_reuse(iters: u64, cap_override: Option<usize>) {
    let batch_len = cap_override.unwrap_or(1_000);
    let batches = build_repeated_batches(iters, 64, batch_len);
    let mut latest_slot: AHashMap<&[u8], usize> = AHashMap::with_capacity(batch_len);
    let mut compacted: Vec<(Vec<u8>, u64)> = Vec::with_capacity(64);
    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for batch in &batches {
        latest_slot.clear();
        let mut used = 0_usize;
        for (key, value) in batch {
            if let Some(&slot) = latest_slot.get(key.as_slice()) {
                if let Some((_, current_value)) = compacted.get_mut(slot) {
                    *current_value = *value;
                }
                continue;
            }

            latest_slot.insert(key.as_slice(), used);
            if used == compacted.len() {
                compacted.push((key.clone(), *value));
            } else if let Some((scratch_key, scratch_value)) = compacted.get_mut(used) {
                scratch_key.clear();
                scratch_key.extend_from_slice(key);
                *scratch_value = *value;
            }
            used += 1;
        }
        total = total.wrapping_add(used);
        std::hint::black_box(&compacted[..used]);
    }
    let elapsed = start.elapsed();
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
}

fn run_compact_reuse_plus_update_vec_box_prebuilt(iters: u64, cap_override: Option<usize>) {
    let batch_len = cap_override.unwrap_or(1_000);
    let batches = build_repeated_batches(iters, 64, batch_len);
    let seed_batch = build_compacted_repeated_batches(1, batch_len)
        .into_iter()
        .next()
        .unwrap_or_default();
    let map = ConcurrentHashMap::<Vec<u8>, Box<(u64, u64)>, AHashState>::with_capacity_and_hasher(
        1024,
        AHashState::new(),
    );
    let reserved = map.reserve(seed_batch.len());
    drop(reserved);
    for (offset, (key, value)) in seed_batch.into_iter().enumerate() {
        let inserted = map.insert_sync(key, Box::new((offset as u64, value)));
        let _ = std::hint::black_box(inserted);
    }

    let mut latest_slot: AHashMap<&[u8], usize> = AHashMap::with_capacity(batch_len);
    let mut compacted: Vec<(Vec<u8>, u64)> = Vec::with_capacity(64);
    let mut next_id = 0_u64;
    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for batch in &batches {
        latest_slot.clear();
        let mut used = 0_usize;
        for (key, value) in batch {
            if let Some(&slot) = latest_slot.get(key.as_slice()) {
                if let Some((_, current_value)) = compacted.get_mut(slot) {
                    *current_value = *value;
                }
                continue;
            }

            latest_slot.insert(key.as_slice(), used);
            if used == compacted.len() {
                compacted.push((key.clone(), *value));
            } else if let Some((scratch_key, scratch_value)) = compacted.get_mut(used) {
                scratch_key.clear();
                scratch_key.extend_from_slice(key);
                *scratch_value = *value;
            }
            used += 1;
        }

        for (key, value) in compacted[..used].iter() {
            let id = next_id;
            next_id = next_id.wrapping_add(1);
            if map
                .update_sync(key.as_slice(), |_, current| {
                    current.0 = id;
                    current.1 = *value;
                })
                .is_some()
            {
                total = total.wrapping_add(1);
                continue;
            }

            let inserted = map.insert_sync(key.clone(), Box::new((id, *value)));
            let _ = std::hint::black_box(inserted);
        }
    }
    let elapsed = start.elapsed();
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
}

fn build_compacted_repeated_batches(iters: u64, batch_len: usize) -> Vec<Vec<(Vec<u8>, u64)>> {
    build_repeated_batches(iters, 64, batch_len)
        .into_iter()
        .map(compact_last_write_wins_owned)
        .collect()
}

fn run_clone_keys_repeated_compacted_prebuilt(iters: u64, cap_override: Option<usize>) {
    let batch_len = cap_override.unwrap_or(1_000);
    let batches = build_compacted_repeated_batches(iters, batch_len);
    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for batch in batches {
        let clones = batch
            .into_iter()
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        total = total.wrapping_add(clones.len());
        std::hint::black_box(clones);
    }
    let elapsed = start.elapsed();
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
}

fn run_consume_repeated_compacted_prebuilt(iters: u64, cap_override: Option<usize>) {
    let batch_len = cap_override.unwrap_or(1_000);
    let batches = build_compacted_repeated_batches(iters, batch_len);
    maybe_profile_delay();
    let start = Instant::now();
    let mut acc = 0_u64;
    for batch in batches {
        for (key, value) in batch {
            acc ^= value ^ key.len() as u64;
            std::hint::black_box((key, value));
        }
    }
    let elapsed = start.elapsed();
    std::hint::black_box(acc);
    std::hint::black_box(elapsed);
}

fn run_hash_keys_repeated_compacted_prebuilt(iters: u64, cap_override: Option<usize>) {
    let batch_len = cap_override.unwrap_or(1_000);
    let batches = build_compacted_repeated_batches(iters, batch_len);
    let hasher = AHashState::new();
    maybe_profile_delay();
    let start = Instant::now();
    let mut acc = 0_u64;
    for batch in batches {
        for (key, _) in batch {
            acc ^= hasher.hash_one(&key);
        }
    }
    let elapsed = start.elapsed();
    std::hint::black_box(acc);
    std::hint::black_box(elapsed);
}

fn run_box_records_repeated_compacted_prebuilt(iters: u64, cap_override: Option<usize>) {
    let batch_len = cap_override.unwrap_or(1_000);
    let batches = build_compacted_repeated_batches(iters, batch_len);
    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for batch in batches {
        let boxed = batch
            .into_iter()
            .enumerate()
            .map(|(offset, (_, value))| Box::new((offset as u64, value)))
            .collect::<Vec<_>>();
        total = total.wrapping_add(boxed.len());
        std::hint::black_box(boxed);
    }
    let elapsed = start.elapsed();
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
}

fn run_clone_keys_repeated_compacted_borrowed(iters: u64, cap_override: Option<usize>) {
    let batch_len = cap_override.unwrap_or(1_000);
    let batches = build_compacted_repeated_batches(iters, batch_len);
    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for batch in &batches {
        let clones = batch.iter().map(|(key, _)| key.clone()).collect::<Vec<_>>();
        total = total.wrapping_add(clones.len());
        std::hint::black_box(clones);
    }
    let elapsed = start.elapsed();
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
}

fn run_hash_keys_repeated_compacted_borrowed(iters: u64, cap_override: Option<usize>) {
    let batch_len = cap_override.unwrap_or(1_000);
    let batches = build_compacted_repeated_batches(iters, batch_len);
    let hasher = AHashState::new();
    maybe_profile_delay();
    let start = Instant::now();
    let mut acc = 0_u64;
    for batch in &batches {
        for (key, _) in batch {
            acc ^= hasher.hash_one(key);
        }
    }
    let elapsed = start.elapsed();
    std::hint::black_box(acc);
    std::hint::black_box(elapsed);
}

fn run_box_records_repeated_compacted_borrowed(iters: u64, cap_override: Option<usize>) {
    let batch_len = cap_override.unwrap_or(1_000);
    let batches = build_compacted_repeated_batches(iters, batch_len);
    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for batch in &batches {
        let boxed = batch
            .iter()
            .enumerate()
            .map(|(offset, (_, value))| Box::new((offset as u64, *value)))
            .collect::<Vec<_>>();
        total = total.wrapping_add(boxed.len());
        std::hint::black_box(boxed);
    }
    let elapsed = start.elapsed();
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
}

fn run_clone_and_box_repeated_compacted_borrowed(iters: u64, cap_override: Option<usize>) {
    let batch_len = cap_override.unwrap_or(1_000);
    let batches = build_compacted_repeated_batches(iters, batch_len);
    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for batch in &batches {
        let prepared = batch
            .iter()
            .enumerate()
            .map(|(offset, (key, value))| (key.clone(), Box::new((offset as u64, *value))))
            .collect::<Vec<_>>();
        total = total.wrapping_add(prepared.len());
        std::hint::black_box(prepared);
    }
    let elapsed = start.elapsed();
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
}

fn run_clone_and_box_repeated_compacted_borrowed_leak(iters: u64, cap_override: Option<usize>) {
    let batch_len = cap_override.unwrap_or(1_000);
    let batches = build_compacted_repeated_batches(iters, batch_len);
    maybe_profile_delay();
    let start = Instant::now();
    let mut prepared_batches = Vec::with_capacity(batches.len());
    let mut total = 0_usize;
    for batch in &batches {
        let prepared = batch
            .iter()
            .enumerate()
            .map(|(offset, (key, value))| (key.clone(), Box::new((offset as u64, *value))))
            .collect::<Vec<_>>();
        total = total.wrapping_add(prepared.len());
        prepared_batches.push(prepared);
    }
    let elapsed = start.elapsed();
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
    std::mem::forget(prepared_batches);
}

fn run_convert_keys_to_vec_repeated_compacted_borrowed(iters: u64, cap_override: Option<usize>) {
    let batch_len = cap_override.unwrap_or(1_000);
    let batches = build_compacted_repeated_batches(iters, batch_len);
    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for batch in &batches {
        let converted = batch
            .iter()
            .map(|(key, _)| key.to_vec())
            .collect::<Vec<Vec<u8>>>();
        total = total.wrapping_add(converted.len());
        std::hint::black_box(converted);
    }
    let elapsed = start.elapsed();
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
}

fn run_convert_keys_to_arc_slice_repeated_compacted_borrowed(
    iters: u64,
    cap_override: Option<usize>,
) {
    let batch_len = cap_override.unwrap_or(1_000);
    let batches = build_compacted_repeated_batches(iters, batch_len);
    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for batch in &batches {
        let converted = batch
            .iter()
            .map(|(key, _)| Arc::<[u8]>::from(key.as_slice()))
            .collect::<Vec<Arc<[u8]>>>();
        total = total.wrapping_add(converted.len());
        std::hint::black_box(converted);
    }
    let elapsed = start.elapsed();
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
}

fn run_convert_keys_to_bytes_repeated_compacted_borrowed(iters: u64, cap_override: Option<usize>) {
    let batch_len = cap_override.unwrap_or(1_000);
    let batches = build_compacted_repeated_batches(iters, batch_len);
    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for batch in &batches {
        let converted = batch
            .iter()
            .map(|(key, _)| Bytes::copy_from_slice(key))
            .collect::<Vec<Bytes>>();
        total = total.wrapping_add(converted.len());
        std::hint::black_box(converted);
    }
    let elapsed = start.elapsed();
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
}

fn build_prepared_compacted_batches(iters: u64, batch_len: usize) -> Vec<PreparedCompactedBatch> {
    build_compacted_repeated_batches(iters, batch_len)
        .into_iter()
        .map(|batch| {
            batch
                .into_iter()
                .enumerate()
                .map(|(offset, (key, value))| (key, Box::new((offset as u64, value))))
                .collect()
        })
        .collect()
}

fn build_compacted_repeated_u64_batches(
    iters: u64,
    hot_keys: usize,
    batch_len: usize,
) -> Vec<Vec<(u64, u64)>> {
    let hot_keys = hot_keys as u64;
    let mut batches = Vec::with_capacity(iters as usize);
    let mut seq = 0_u64;
    for _ in 0..iters {
        let mut latest_slot = AHashMap::with_capacity(batch_len);
        let mut compacted = Vec::with_capacity(batch_len);
        for i in 0..batch_len {
            let key = (i as u64) % hot_keys;
            let value = seq;
            seq = seq.wrapping_add(1);
            if let Some(&slot) = latest_slot.get(&key) {
                if let Some((_, current_value)) = compacted.get_mut(slot) {
                    *current_value = value;
                }
            } else {
                latest_slot.insert(key, compacted.len());
                compacted.push((key, value));
            }
        }
        batches.push(compacted);
    }
    batches
}

fn build_compacted_repeated_array8_batches(
    iters: u64,
    hot_keys: usize,
    batch_len: usize,
) -> Vec<Vec<([u8; 8], u64)>> {
    let hot_keys = hot_keys as u64;
    let mut batches = Vec::with_capacity(iters as usize);
    let mut seq = 0_u64;
    for _ in 0..iters {
        let mut latest_slot = AHashMap::with_capacity(batch_len);
        let mut compacted = Vec::with_capacity(batch_len);
        for i in 0..batch_len {
            let key = ((i as u64) % hot_keys).to_le_bytes();
            let value = seq;
            seq = seq.wrapping_add(1);
            if let Some(&slot) = latest_slot.get(&key) {
                if let Some((_, current_value)) = compacted.get_mut(slot) {
                    *current_value = value;
                }
            } else {
                latest_slot.insert(key, compacted.len());
                compacted.push((key, value));
            }
        }
        batches.push(compacted);
    }
    batches
}

fn build_compacted_repeated_array16_batches(
    iters: u64,
    hot_keys: usize,
    batch_len: usize,
) -> Vec<Vec<([u8; 16], u64)>> {
    let hot_keys = hot_keys as u64;
    let mut batches = Vec::with_capacity(iters as usize);
    let mut seq = 0_u64;
    for _ in 0..iters {
        let mut latest_slot = AHashMap::with_capacity(batch_len);
        let mut compacted = Vec::with_capacity(batch_len);
        for i in 0..batch_len {
            let mut key = [0_u8; 16];
            key[..8].copy_from_slice(&((i as u64) % hot_keys).to_le_bytes());
            key[8..].copy_from_slice(&(0xfeed_face_cafe_beefu64).to_le_bytes());
            let value = seq;
            seq = seq.wrapping_add(1);
            if let Some(&slot) = latest_slot.get(&key) {
                if let Some((_, current_value)) = compacted.get_mut(slot) {
                    *current_value = value;
                }
            } else {
                latest_slot.insert(key, compacted.len());
                compacted.push((key, value));
            }
        }
        batches.push(compacted);
    }
    batches
}

fn build_compacted_repeated_boxed_array8_batches(
    iters: u64,
    hot_keys: usize,
    batch_len: usize,
) -> Vec<Vec<(Box<[u8; 8]>, u64)>> {
    build_compacted_repeated_array8_batches(iters, hot_keys, batch_len)
        .into_iter()
        .map(|batch| {
            batch
                .into_iter()
                .map(|(key, value)| (Box::new(key), value))
                .collect()
        })
        .collect()
}

fn build_compacted_repeated_arc_slice_batches(
    iters: u64,
    hot_keys: usize,
    batch_len: usize,
) -> Vec<Vec<(Arc<[u8]>, u64)>> {
    build_compacted_repeated_array8_batches(iters, hot_keys, batch_len)
        .into_iter()
        .map(|batch| {
            batch
                .into_iter()
                .map(|(key, value)| (Arc::<[u8]>::from(key.to_vec()), value))
                .collect()
        })
        .collect()
}

fn build_compacted_repeated_bytes_batches(
    iters: u64,
    hot_keys: usize,
    batch_len: usize,
) -> Vec<Vec<(Bytes, u64)>> {
    build_compacted_repeated_array8_batches(iters, hot_keys, batch_len)
        .into_iter()
        .map(|batch| {
            batch
                .into_iter()
                .map(|(key, value)| (Bytes::copy_from_slice(&key), value))
                .collect()
        })
        .collect()
}

fn run_scc_upsert_overwrite_prepared(iters: u64, cap_override: Option<usize>, forget_old: bool) {
    let batch_len = cap_override.unwrap_or(1_000);
    let prepared_batches = build_prepared_compacted_batches(iters, batch_len);
    let seed_batch = build_compacted_repeated_batches(1, batch_len)
        .into_iter()
        .next()
        .unwrap_or_default();
    let map = ConcurrentHashMap::<Vec<u8>, Box<(u64, u64)>, AHashState>::with_capacity_and_hasher(
        1024,
        AHashState::new(),
    );
    let reserved = map.reserve(seed_batch.len());
    drop(reserved);
    for (offset, (key, value)) in seed_batch.into_iter().enumerate() {
        let previous = map.upsert_sync(key, Box::new((offset as u64, value)));
        std::hint::black_box(previous);
    }

    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for batch in prepared_batches {
        for (key, record) in batch {
            let previous = map.upsert_sync(key, record);
            if let Some(old) = previous {
                total = total.wrapping_add(1);
                if forget_old {
                    std::mem::forget(old);
                } else {
                    std::hint::black_box(old);
                }
            }
        }
    }
    let elapsed = start.elapsed();
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
}

fn run_scc_upsert_overwrite_bytes_u64_prepared(iters: u64, cap_override: Option<usize>) {
    let batch_len = cap_override.unwrap_or(1_000);
    let prepared_batches = build_compacted_repeated_batches(iters, batch_len);
    let seed_batch = build_compacted_repeated_batches(1, batch_len)
        .into_iter()
        .next()
        .unwrap_or_default();
    let map = ConcurrentHashMap::<Vec<u8>, u64, AHashState>::with_capacity_and_hasher(
        1024,
        AHashState::new(),
    );
    let reserved = map.reserve(seed_batch.len());
    drop(reserved);
    for (key, value) in seed_batch {
        let previous = map.upsert_sync(key, value);
        std::hint::black_box(previous);
    }

    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for batch in prepared_batches {
        for (key, value) in batch {
            let previous = map.upsert_sync(key, value);
            if previous.is_some() {
                total = total.wrapping_add(1);
            }
        }
    }
    let elapsed = start.elapsed();
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
}

fn run_scc_upsert_overwrite_vec_u64_from_borrowed(iters: u64, cap_override: Option<usize>) {
    let batch_len = cap_override.unwrap_or(1_000);
    let batches = build_compacted_repeated_batches(iters, batch_len);
    let seed_batch = build_compacted_repeated_batches(1, batch_len)
        .into_iter()
        .next()
        .unwrap_or_default();
    let map = ConcurrentHashMap::<Vec<u8>, u64, AHashState>::with_capacity_and_hasher(
        1024,
        AHashState::new(),
    );
    let reserved = map.reserve(seed_batch.len());
    drop(reserved);
    for (key, value) in seed_batch {
        let previous = map.upsert_sync(key, value);
        std::hint::black_box(previous);
    }

    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for batch in &batches {
        for (key, value) in batch {
            let previous = map.upsert_sync(key.to_vec(), *value);
            if previous.is_some() {
                total = total.wrapping_add(1);
            }
        }
    }
    let elapsed = start.elapsed();
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
}

fn run_scc_update_or_insert_vec_u64_from_borrowed(iters: u64, cap_override: Option<usize>) {
    let batch_len = cap_override.unwrap_or(1_000);
    let batches = build_compacted_repeated_batches(iters, batch_len);
    let seed_batch = build_compacted_repeated_batches(1, batch_len)
        .into_iter()
        .next()
        .unwrap_or_default();
    let map = ConcurrentHashMap::<Vec<u8>, u64, AHashState>::with_capacity_and_hasher(
        1024,
        AHashState::new(),
    );
    let reserved = map.reserve(seed_batch.len());
    drop(reserved);
    for (key, value) in seed_batch {
        let inserted = map.insert_sync(key, value);
        let _ = std::hint::black_box(inserted);
    }

    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for batch in &batches {
        for (key, value) in batch {
            if map
                .update_sync(key.as_slice(), |_, current| {
                    *current = *value;
                })
                .is_some()
            {
                total = total.wrapping_add(1);
                continue;
            }

            let inserted = map.insert_sync(key.clone(), *value);
            let _ = std::hint::black_box(inserted);
        }
    }
    let elapsed = start.elapsed();
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
}

fn run_scc_upsert_overwrite_vec_box_from_borrowed(iters: u64, cap_override: Option<usize>) {
    let batch_len = cap_override.unwrap_or(1_000);
    let batches = build_compacted_repeated_batches(iters, batch_len);
    let seed_batch = build_compacted_repeated_batches(1, batch_len)
        .into_iter()
        .next()
        .unwrap_or_default();
    let map = ConcurrentHashMap::<Vec<u8>, Box<(u64, u64)>, AHashState>::with_capacity_and_hasher(
        1024,
        AHashState::new(),
    );
    let reserved = map.reserve(seed_batch.len());
    drop(reserved);
    for (offset, (key, value)) in seed_batch.into_iter().enumerate() {
        let previous = map.upsert_sync(key, Box::new((offset as u64, value)));
        std::hint::black_box(previous);
    }

    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for batch in &batches {
        for (offset, (key, value)) in batch.iter().enumerate() {
            let previous = map.upsert_sync(key.to_vec(), Box::new((offset as u64, *value)));
            if previous.is_some() {
                total = total.wrapping_add(1);
            }
        }
    }
    let elapsed = start.elapsed();
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
}

fn run_scc_update_or_insert_vec_box_from_borrowed(iters: u64, cap_override: Option<usize>) {
    let batch_len = cap_override.unwrap_or(1_000);
    let batches = build_compacted_repeated_batches(iters, batch_len);
    let seed_batch = build_compacted_repeated_batches(1, batch_len)
        .into_iter()
        .next()
        .unwrap_or_default();
    let map = ConcurrentHashMap::<Vec<u8>, Box<(u64, u64)>, AHashState>::with_capacity_and_hasher(
        1024,
        AHashState::new(),
    );
    let reserved = map.reserve(seed_batch.len());
    drop(reserved);
    for (offset, (key, value)) in seed_batch.into_iter().enumerate() {
        let inserted = map.insert_sync(key, Box::new((offset as u64, value)));
        let _ = std::hint::black_box(inserted);
    }

    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for batch in &batches {
        for (offset, (key, value)) in batch.iter().enumerate() {
            if map
                .update_sync(key.as_slice(), |_, current| {
                    current.0 = offset as u64;
                    current.1 = *value;
                })
                .is_some()
            {
                total = total.wrapping_add(1);
                continue;
            }

            let inserted = map.insert_sync(key.clone(), Box::new((offset as u64, *value)));
            let _ = std::hint::black_box(inserted);
        }
    }
    let elapsed = start.elapsed();
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
}

fn run_scc_upsert_overwrite_arc_slice_u64_from_borrowed(iters: u64, cap_override: Option<usize>) {
    let batch_len = cap_override.unwrap_or(1_000);
    let batches = build_compacted_repeated_batches(iters, batch_len);
    let seed_batch = build_compacted_repeated_arc_slice_batches(1, 64, batch_len)
        .into_iter()
        .next()
        .unwrap_or_default();
    let map = ConcurrentHashMap::<Arc<[u8]>, u64, AHashState>::with_capacity_and_hasher(
        1024,
        AHashState::new(),
    );
    let reserved = map.reserve(seed_batch.len());
    drop(reserved);
    for (key, value) in seed_batch {
        let previous = map.upsert_sync(key, value);
        std::hint::black_box(previous);
    }

    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for batch in &batches {
        for (key, value) in batch {
            let previous = map.upsert_sync(Arc::<[u8]>::from(key.as_slice()), *value);
            if previous.is_some() {
                total = total.wrapping_add(1);
            }
        }
    }
    let elapsed = start.elapsed();
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
}

fn run_scc_upsert_overwrite_bytes_crate_u64_from_borrowed(iters: u64, cap_override: Option<usize>) {
    let batch_len = cap_override.unwrap_or(1_000);
    let batches = build_compacted_repeated_batches(iters, batch_len);
    let seed_batch = build_compacted_repeated_bytes_batches(1, 64, batch_len)
        .into_iter()
        .next()
        .unwrap_or_default();
    let map = ConcurrentHashMap::<Bytes, u64, AHashState>::with_capacity_and_hasher(
        1024,
        AHashState::new(),
    );
    let reserved = map.reserve(seed_batch.len());
    drop(reserved);
    for (key, value) in seed_batch {
        let previous = map.upsert_sync(key, value);
        std::hint::black_box(previous);
    }

    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for batch in &batches {
        for (key, value) in batch {
            let previous = map.upsert_sync(Bytes::copy_from_slice(key), *value);
            if previous.is_some() {
                total = total.wrapping_add(1);
            }
        }
    }
    let elapsed = start.elapsed();
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
}

fn run_scc_upsert_overwrite_u64_u64_prepared(iters: u64, cap_override: Option<usize>) {
    let batch_len = cap_override.unwrap_or(1_000);
    let prepared_batches = build_compacted_repeated_u64_batches(iters, 64, batch_len);
    let seed_batch = build_compacted_repeated_u64_batches(1, 64, batch_len)
        .into_iter()
        .next()
        .unwrap_or_default();
    let map = ConcurrentHashMap::<u64, u64, AHashState>::with_capacity_and_hasher(
        1024,
        AHashState::new(),
    );
    let reserved = map.reserve(seed_batch.len());
    drop(reserved);
    for (key, value) in seed_batch {
        let previous = map.upsert_sync(key, value);
        std::hint::black_box(previous);
    }

    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for batch in prepared_batches {
        for (key, value) in batch {
            let previous = map.upsert_sync(key, value);
            if previous.is_some() {
                total = total.wrapping_add(1);
            }
        }
    }
    let elapsed = start.elapsed();
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
}

fn run_scc_upsert_overwrite_u64_box_prepared(iters: u64, cap_override: Option<usize>) {
    let batch_len = cap_override.unwrap_or(1_000);
    let prepared_batches = build_compacted_repeated_u64_batches(iters, 64, batch_len)
        .into_iter()
        .map(|batch| {
            batch
                .into_iter()
                .enumerate()
                .map(|(offset, (key, value))| (key, Box::new((offset as u64, value))))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let seed_batch = build_compacted_repeated_u64_batches(1, 64, batch_len)
        .into_iter()
        .next()
        .unwrap_or_default();
    let map = ConcurrentHashMap::<u64, Box<(u64, u64)>, AHashState>::with_capacity_and_hasher(
        1024,
        AHashState::new(),
    );
    let reserved = map.reserve(seed_batch.len());
    drop(reserved);
    for (offset, (key, value)) in seed_batch.into_iter().enumerate() {
        let previous = map.upsert_sync(key, Box::new((offset as u64, value)));
        std::hint::black_box(previous);
    }

    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for batch in prepared_batches {
        for (key, value) in batch {
            let previous = map.upsert_sync(key, value);
            if previous.is_some() {
                total = total.wrapping_add(1);
            }
        }
    }
    let elapsed = start.elapsed();
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
}

fn run_scc_upsert_overwrite_arc_slice_u64_prepared(iters: u64, cap_override: Option<usize>) {
    let batch_len = cap_override.unwrap_or(1_000);
    let prepared_batches = build_compacted_repeated_arc_slice_batches(iters, 64, batch_len);
    let seed_batch = build_compacted_repeated_arc_slice_batches(1, 64, batch_len)
        .into_iter()
        .next()
        .unwrap_or_default();
    let map = ConcurrentHashMap::<Arc<[u8]>, u64, AHashState>::with_capacity_and_hasher(
        1024,
        AHashState::new(),
    );
    let reserved = map.reserve(seed_batch.len());
    drop(reserved);
    for (key, value) in seed_batch {
        let previous = map.upsert_sync(key, value);
        std::hint::black_box(previous);
    }

    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for batch in prepared_batches {
        for (key, value) in batch {
            let previous = map.upsert_sync(key, value);
            if previous.is_some() {
                total = total.wrapping_add(1);
            }
        }
    }
    let elapsed = start.elapsed();
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
}

fn run_scc_upsert_overwrite_bytes_crate_u64_prepared(iters: u64, cap_override: Option<usize>) {
    let batch_len = cap_override.unwrap_or(1_000);
    let prepared_batches = build_compacted_repeated_bytes_batches(iters, 64, batch_len);
    let seed_batch = build_compacted_repeated_bytes_batches(1, 64, batch_len)
        .into_iter()
        .next()
        .unwrap_or_default();
    let map = ConcurrentHashMap::<Bytes, u64, AHashState>::with_capacity_and_hasher(
        1024,
        AHashState::new(),
    );
    let reserved = map.reserve(seed_batch.len());
    drop(reserved);
    for (key, value) in seed_batch {
        let previous = map.upsert_sync(key, value);
        std::hint::black_box(previous);
    }

    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for batch in prepared_batches {
        for (key, value) in batch {
            let previous = map.upsert_sync(key, value);
            if previous.is_some() {
                total = total.wrapping_add(1);
            }
        }
    }
    let elapsed = start.elapsed();
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
}

fn run_scc_upsert_overwrite_array8_u64_prepared(iters: u64, cap_override: Option<usize>) {
    let batch_len = cap_override.unwrap_or(1_000);
    let prepared_batches = build_compacted_repeated_array8_batches(iters, 64, batch_len);
    let seed_batch = build_compacted_repeated_array8_batches(1, 64, batch_len)
        .into_iter()
        .next()
        .unwrap_or_default();
    let map = ConcurrentHashMap::<[u8; 8], u64, AHashState>::with_capacity_and_hasher(
        1024,
        AHashState::new(),
    );
    let reserved = map.reserve(seed_batch.len());
    drop(reserved);
    for (key, value) in seed_batch {
        let previous = map.upsert_sync(key, value);
        std::hint::black_box(previous);
    }

    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for batch in prepared_batches {
        for (key, value) in batch {
            let previous = map.upsert_sync(key, value);
            if previous.is_some() {
                total = total.wrapping_add(1);
            }
        }
    }
    let elapsed = start.elapsed();
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
}

fn run_scc_upsert_overwrite_array16_u64_prepared(iters: u64, cap_override: Option<usize>) {
    let batch_len = cap_override.unwrap_or(1_000);
    let prepared_batches = build_compacted_repeated_array16_batches(iters, 64, batch_len);
    let seed_batch = build_compacted_repeated_array16_batches(1, 64, batch_len)
        .into_iter()
        .next()
        .unwrap_or_default();
    let map = ConcurrentHashMap::<[u8; 16], u64, AHashState>::with_capacity_and_hasher(
        1024,
        AHashState::new(),
    );
    let reserved = map.reserve(seed_batch.len());
    drop(reserved);
    for (key, value) in seed_batch {
        let previous = map.upsert_sync(key, value);
        std::hint::black_box(previous);
    }

    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for batch in prepared_batches {
        for (key, value) in batch {
            let previous = map.upsert_sync(key, value);
            if previous.is_some() {
                total = total.wrapping_add(1);
            }
        }
    }
    let elapsed = start.elapsed();
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
}

fn run_scc_upsert_overwrite_boxed_array8_u64_prepared(iters: u64, cap_override: Option<usize>) {
    let batch_len = cap_override.unwrap_or(1_000);
    let prepared_batches = build_compacted_repeated_boxed_array8_batches(iters, 64, batch_len);
    let seed_batch = build_compacted_repeated_boxed_array8_batches(1, 64, batch_len)
        .into_iter()
        .next()
        .unwrap_or_default();
    let map = ConcurrentHashMap::<Box<[u8; 8]>, u64, AHashState>::with_capacity_and_hasher(
        1024,
        AHashState::new(),
    );
    let reserved = map.reserve(seed_batch.len());
    drop(reserved);
    for (key, value) in seed_batch {
        let previous = map.upsert_sync(key, value);
        std::hint::black_box(previous);
    }

    maybe_profile_delay();
    let start = Instant::now();
    let mut total = 0_usize;
    for batch in prepared_batches {
        for (key, value) in batch {
            let previous = map.upsert_sync(key, value);
            if previous.is_some() {
                total = total.wrapping_add(1);
            }
        }
    }
    let elapsed = start.elapsed();
    std::hint::black_box(total);
    std::hint::black_box(elapsed);
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

fn run_flush_batch_build_unique(iters: u64, cap_override: Option<usize>) {
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
    let batch = map.low_level().flush_batch(1_000_000);
    let elapsed = start.elapsed();
    std::mem::forget(map);
    std::hint::black_box(batch);
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
