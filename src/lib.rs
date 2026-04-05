//! `cachelog-rs` provides a concurrent visible-index map backed by an ordered dirty write log and
//! an optional clean cache store.
//!
//! The core model is:
//!
//! - one concurrent visible index keyed by `K`
//! - dirty writes append to an ordered log and install a visible `Dirty` ref
//! - clean cache entries install a visible `Clean` ref
//! - reads consult only the visible index and resolve the underlying record
//! - dirty refs are held strongly in the visible index until flusher-side conditional cleanup
//!
//! Flush ordering is derived from the dirty log rather than by scanning the visible index.
//!
//! Current implementation detail:
//!
//! - visible dirty entries hold the same `Arc<DirtyRecord>` as the dirty FIFO
//! - `mark_flushed()` conditionally removes only the exact flushed visible record
//! - clean eviction removes the visible clean entry immediately
//!
//! The TLA+/PlusCal models under `models/` and `model-cachelog/` intentionally remain a slightly
//! looser semantic envelope: they allow split drop/cleanup actions to explore more interleavings
//! than the live crate currently exposes.

mod sync;
mod entry;
mod map;

pub use entry::{CacheId, CleanRecord, DirtyRecord, EntryState, FlushBatch, VisibleRef, WriteId};
pub use map::{CacheLogConfig, CacheLogMap};
