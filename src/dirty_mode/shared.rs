//! Shared dirty-mode types used by both native and loom implementations.

use crate::entry::FlushBatch;

pub(crate) enum FlushWork<K, V> {
    Batch(FlushBatch<K, V>),
    ForceFlush,
}
