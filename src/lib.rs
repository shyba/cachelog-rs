//! `cachelog-rs` provides a concurrent visible-index map backed by an ordered dirty write log and
//! an optional clean cache store.
//!
//! The core model is:
//!
//! - one concurrent visible index keyed by `K`
//! - dirty writes append to an ordered log and install a visible `Dirty` ref
//! - clean cache entries install a visible `Clean` ref
//! - reads consult only the visible index and resolve the underlying record
//! - dirty refs are held in the visible index until flusher-side conditional cleanup
//!
//! Flush ordering is derived from the dirty log rather than by scanning the visible index.
//!
//! Current implementation detail:
//!
//! - visible dirty entries are stored directly in the visible map
//! - dirty flush order is maintained by an explicit ordered FIFO dirty mode
//! - `mark_flushed()` conditionally removes only the exact flushed visible record
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
//! - `cachelog-rs` is validated against that model with conformance tests and loom scenarios
//! - the live concurrent implementation is not deductively verified end to end

mod sync;
mod dirty_mode;
mod entry;
mod map;

pub use entry::{CacheId, CleanRecord, DirtyRecord, EntryState, FlushBatch, VisibleRef, WriteId};
pub use map::{CacheLogConfig, CacheLogMap};
#[cfg(feature = "loom")]
pub use map::{DebugRecord, DebugSnapshot, DebugVisible};
