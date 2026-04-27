//! `cachelog-rs` provides a concurrent visible-index map with an optional clean cache store.
//!
//! Dirty-write behavior is mode-driven:
//!
//! - `DirtyWriteMode::StrictLog`: dirty writes append to an ordered log, flush order follows that log
//! - `DirtyWriteMode::CoalescedMap`: dirty writes publish into a latest-value overlay, and flush
//!   persists detached snapshots of that overlay (latest-value throughput mode)
//!
//! Shared core behavior:
//!
//! - one concurrent visible index keyed by `K`
//! - clean cache entries install a visible `Clean` ref
//! - common reads return `(value, EntryState)` and do not require dirty-record ids
//! - prefix/list reads use `with_prefix_snapshot(...)` and do not force persistence
//! - common persist helpers use `PersistBatch`, not id-bearing dirty records
//! - persisted-consistent scans use `with_persisted_scan(...)`
//! - clean eviction removes the visible clean entry immediately
//! - low-level strict/debug helpers live under `cachelog::low_level`
//!
//! The TLA+/PlusCal source models under `models/` intentionally remain a slightly looser semantic
//! envelope: they allow split drop/cleanup actions to explore more interleavings than the live
//! crate currently exposes. The active trace-validation spec consumed by `model-cachelog` lives
//! separately under `model-cachelog/formal/`.
//!
//! Formal proof boundary:
//!
//! - `cachelog-core` proves the deterministic semantic state machine
//! - `cachelog-rs` `StrictLog` mode is validated against that model with conformance tests and loom scenarios
//! - `CoalescedMap` is tested for safety/invariants but intentionally relaxes ordered-flush semantics
//! - the live concurrent implementation is not deductively verified end to end
//!
mod background_flush;
mod byte_prefix_map;
#[cfg(any(test, feature = "dev-tools"))]
mod bytes_pooling;
mod dirty_mode;
mod entry;
mod map;
#[cfg(feature = "dev-tools")]
mod perf;
mod sync;

#[cfg(not(feature = "loom"))]
pub use background_flush::{BackgroundFlushConfig, BackgroundFlushHandle};
pub use byte_prefix_map::BytePrefixMap;
pub use entry::{EntryState, PersistBatch, PersistBatchEntry};
pub use map::{CacheLogConfig, CacheLogMap, DirtyBacklogCounts, DirtyWriteMode, FlushWork};
#[cfg(feature = "loom")]
pub use map::{DebugRecord, DebugSnapshot, DebugVisible};
#[cfg(feature = "dev-tools")]
pub use perf::CoalescedSingleWritePerfCounters;

pub mod low_level {
    pub use crate::entry::{CacheId, CleanRecord, DirtyRecord, FlushBatch, VisibleRef, WriteId};
    pub use crate::map::LowLevelMap;
}
