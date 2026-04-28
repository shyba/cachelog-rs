//! Dirty-mode queue implementation split across three files:
//! - `shared.rs`  — FlushWork enum shared by all implementations
//! - `native.rs`  — crossbeam-channel based implementation (default)
//! - `loom.rs`    — loom-based implementation (when feature = "loom")

#[cfg(feature = "loom")]
pub(crate) mod loom;
#[cfg(not(feature = "loom"))]
pub(crate) mod native;
pub(crate) mod shared;

// Re-export public types so callers can use `crate::dirty_mode::{DirtyMode, ...}`
#[cfg(feature = "loom")]
pub(crate) use loom::{DirtyMode, OrderedFifoDirty};
#[cfg(not(feature = "loom"))]
pub(crate) use native::{DirtyMode, OrderedFifoDirty};
pub(crate) use shared::FlushWork;
