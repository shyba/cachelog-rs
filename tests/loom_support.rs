#![cfg(feature = "loom")]

use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock};

use loom::model::Builder;

use cachelog_rs::{CacheLogMap, DebugSnapshot, DebugVisible, EntryState, VisibleRef};

fn serializer() -> &'static Mutex<()> {
    static SERIALIZER: OnceLock<Mutex<()>> = OnceLock::new();
    SERIALIZER.get_or_init(|| Mutex::new(()))
}

pub const STACK: usize = 4 * 1024 * 1024;

#[allow(dead_code)]
pub fn run_fast_model(f: impl Fn() + Sync + Send + 'static) {
    loom::model(f);
}

#[allow(dead_code)]
pub fn run_exhaustive_model(
    max_threads: usize,
    max_branches: usize,
    max_permutations: usize,
    preemption_bound: Option<usize>,
    f: impl Fn() + Sync + Send + 'static,
) {
    let _guard = serializer()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut builder = Builder::new();
    builder.max_threads = max_threads;
    builder.max_branches = max_branches;
    builder.max_permutations = Some(max_permutations);
    builder.preemption_bound = preemption_bound;
    builder.check(f);
}

#[allow(dead_code)]
pub fn assert_public_consistency(map: &CacheLogMap<usize, usize>, key: usize) {
    let visible = map.visible_ref(&key);
    let read_triplet = map.read(&key, |_, value, state, visible| (*value, state, visible));

    match (visible, read_triplet) {
        (None, None) => {}
        (Some(VisibleRef::Dirty(id1)), Some((_, EntryState::Dirty, VisibleRef::Dirty(id2)))) => {
            assert_eq!(id1, id2, "visible_ref/read dirty id mismatch for key {key}");
        }
        (Some(VisibleRef::Clean(id1)), Some((_, EntryState::Clean, VisibleRef::Clean(id2)))) => {
            assert_eq!(id1, id2, "visible_ref/read clean id mismatch for key {key}");
        }
        (left, right) => panic!("public API mismatch for key {key}: {left:?} vs {right:?}"),
    }

    assert_eq!(
        map.contains(&key),
        read_triplet.is_some(),
        "contains/read mismatch for key {key}"
    );
}

pub fn assert_snapshot_legal(snapshot: &DebugSnapshot<usize, usize>) {
    let mut visible_dirty_ids = BTreeSet::new();
    let mut visible_clean_count = 0usize;

    for (visible_key, visible) in &snapshot.visible {
        match visible {
            DebugVisible::Dirty(record) => {
                assert_eq!(
                    *visible_key, record.key,
                    "visible dirty key mismatch: map key {visible_key}, record key {}",
                    record.key
                );
                assert!(
                    record.id < snapshot.next_write,
                    "visible dirty id {} must be below next_write {}",
                    record.id,
                    snapshot.next_write
                );
                assert!(
                    visible_dirty_ids.insert(record.id),
                    "duplicate visible dirty id {}",
                    record.id
                );
            }
            DebugVisible::Clean(record) => {
                visible_clean_count += 1;
                assert_eq!(
                    *visible_key, record.key,
                    "visible clean key mismatch: map key {visible_key}, record key {}",
                    record.key
                );
                assert!(
                    record.id < snapshot.next_cache,
                    "visible clean id {} must be below next_cache {}",
                    record.id,
                    snapshot.next_cache
                );
            }
        }
    }

    assert_eq!(
        visible_clean_count, snapshot.clean_count,
        "clean_count must match visible clean entries"
    );

    assert_strictly_increasing(
        &snapshot
            .dirty_pending
            .iter()
            .map(|record| record.id)
            .collect::<Vec<_>>(),
        "dirty_pending",
    );
    assert_strictly_increasing(
        &snapshot
            .dirty_inflight
            .iter()
            .map(|record| record.id)
            .collect::<Vec<_>>(),
        "dirty_inflight",
    );

    for record in snapshot
        .dirty_pending
        .iter()
        .chain(snapshot.dirty_inflight.iter())
    {
        assert!(
            record.id < snapshot.next_write,
            "queued dirty id {} must be below next_write {}",
            record.id,
            snapshot.next_write
        );
    }

    let pending_ids = snapshot
        .dirty_pending
        .iter()
        .map(|record| record.id)
        .collect::<BTreeSet<_>>();
    let inflight_ids = snapshot
        .dirty_inflight
        .iter()
        .map(|record| record.id)
        .collect::<BTreeSet<_>>();
    assert!(
        pending_ids.is_disjoint(&inflight_ids),
        "pending and inflight ids must be disjoint"
    );

    if let (Some(max_inflight), Some(min_pending)) = (
        snapshot.dirty_inflight.iter().map(|record| record.id).max(),
        snapshot.dirty_pending.iter().map(|record| record.id).min(),
    ) {
        assert!(
            max_inflight < min_pending,
            "inflight ids must precede pending ids"
        );
    }

    for (_, visible) in &snapshot.visible {
        if let DebugVisible::Dirty(record) = visible {
            assert!(
                pending_ids.contains(&record.id) || inflight_ids.contains(&record.id),
                "visible dirty id {} must still be pending or inflight",
                record.id
            );
        }
    }
}

fn assert_strictly_increasing(ids: &[u64], label: &str) {
    for pair in ids.windows(2) {
        assert!(
            pair[0] < pair[1],
            "{label} is not strictly increasing: {:?}",
            ids
        );
    }
}
