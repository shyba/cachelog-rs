use ahash::AHashMap;

use crate::entry::{CleanRecord, DirtyRecord, FlushBatch, VisibleRef, WriteId};
use crate::sync::Arc;

#[derive(Debug, Clone)]
pub enum VisibleValue<K, V> {
    Dirty(Arc<DirtyRecord<K, V>>),
    Clean(Arc<CleanRecord<K, V>>),
}

impl<K, V> VisibleValue<K, V> {
    pub fn visible_ref(&self) -> VisibleRef {
        match self {
            Self::Dirty(record) => VisibleRef::Dirty(record.id),
            Self::Clean(record) => VisibleRef::Clean(record.id),
        }
    }
}

#[derive(Clone, Debug)]
pub struct CoalescedInflight<K, V> {
    pub(crate) batch: FlushBatch<K, V>,
    pub(crate) index: AHashMap<K, usize>,
}

#[derive(Clone, Debug)]
pub struct CoalescedRecord<V> {
    pub(crate) id: WriteId,
    pub(crate) value: V,
}

pub type CoalescedActiveMap<K, V, H> = scc::HashMap<K, Box<CoalescedRecord<V>>, H>;
pub type SharedCoalescedMap<K, V, H> = Arc<CoalescedActiveMap<K, V, H>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CacheLogConfig {
    pub visible_capacity: usize,
    pub dirty_log_capacity: usize,
    pub clean_capacity: usize,
    pub dirty_write_mode: DirtyWriteMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum DirtyWriteMode {
    #[default]
    StrictLog,
    CoalescedMap,
}

#[derive(Clone, Debug)]
pub enum FlushWork<K, V> {
    Batch(FlushBatch<K, V>),
    ForceFlush,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DirtyBacklogCounts {
    pub dirty_total: usize,
    pub pending_visible: usize,
    pub inflight: usize,
    pub draining: usize,
}

impl<K, V> FlushWork<K, V> {
    pub fn as_batch(&self) -> Option<&FlushBatch<K, V>> {
        match self {
            Self::Batch(batch) => Some(batch),
            Self::ForceFlush => None,
        }
    }
}

impl CacheLogConfig {
    pub const fn new(
        visible_capacity: usize,
        dirty_log_capacity: usize,
        clean_capacity: usize,
    ) -> Self {
        Self {
            visible_capacity,
            dirty_log_capacity,
            clean_capacity,
            dirty_write_mode: DirtyWriteMode::StrictLog,
        }
    }

    pub const fn with_dirty_write_mode(mut self, mode: DirtyWriteMode) -> Self {
        self.dirty_write_mode = mode;
        self
    }
}

impl Default for CacheLogConfig {
    fn default() -> Self {
        Self::new(1024, 1024, 1024)
    }
}
