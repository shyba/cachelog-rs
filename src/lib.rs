//! `cachelog-rs` provides a concurrent visible-index map with an optional clean cache store.
//!
//! Dirty-write behavior is mode-driven:
//!
//! - `DirtyWriteMode::StrictLog`: dirty writes append to an ordered log, flush order follows that log
//! - `DirtyWriteMode::CoalescedMap`: dirty writes publish directly to the visible map, and flush
//!   drains current visible dirty entries (latest-value throughput mode)
//!
//! Shared core behavior:
//!
//! - one concurrent visible index keyed by `K`
//! - clean cache entries install a visible `Clean` ref
//! - reads consult only the visible index and resolve the underlying record
//! - `mark_flushed()` conditionally removes only the exact flushed visible record id
//! - clean eviction removes the visible clean entry immediately
//! - `VisibleRef::Dirty` is the stable logical dirty write id stored in each dirty record
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
mod byte_prefix_map;
mod dirty_mode;
mod entry;
mod map;
mod sync;

pub use byte_prefix_map::BytePrefixMap;
pub use dirty_mode::{DirtyAllocMode, DirtyQueueBackend};
pub use entry::{CacheId, CleanRecord, DirtyRecord, EntryState, FlushBatch, VisibleRef, WriteId};
pub use map::{CacheLogConfig, CacheLogMap, DirtyWriteMode, FlushWork};
#[cfg(feature = "loom")]
pub use map::{DebugRecord, DebugSnapshot, DebugVisible};
