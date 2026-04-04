//! `cachelog-rs` provides a concurrent visible-index map backed by an ordered dirty write log and
//! an optional clean cache store.
//!
//! The core model is:
//!
//! - one concurrent visible index keyed by `K`
//! - dirty writes append to an ordered log and install a visible `Dirty` ref
//! - clean cache entries install a visible `Clean` ref
//! - reads consult only the visible index and resolve the underlying record
//!
//! Flush ordering is derived from the dirty log rather than by scanning the visible index.

mod entry;
mod equivalent;
mod map;

pub use entry::{CacheId, CleanRecord, DirtyRecord, EntryState, FlushBatch, VisibleRef, WriteId};
pub use equivalent::Equivalent;
pub use map::{CacheLogConfig, CacheLogMap};
