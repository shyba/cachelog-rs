mod model;

pub use model::{
    CacheId, CleanRecord, DurableValue, InvariantMode, InvariantReport, KeyId, ModelConfig,
    ModelError, ModelState, ModelStep, ValueId, VisibleRef, WriteId,
};

#[cfg(not(creusot))]
pub use model::{
    ComparableCleanRecord, ComparableDirtyRecord, ComparableDurableValue, ComparableState,
    ComparableVisibleRef,
};

#[cfg(feature = "creusot")]
pub mod proofs;

#[cfg(kani)]
mod kani;
