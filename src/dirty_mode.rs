use std::collections::VecDeque;

use crate::entry::{DirtyRecord, FlushBatch};
use crate::entry::WriteId;
use crate::sync::{Arc, Mutex, lock, new_mutex};

pub(crate) trait DirtyMode<K, V> {
    type VisibleDirty: Clone;

    fn new(capacity: usize) -> Self;
    fn append(&self, key: K, value: V) -> Self::VisibleDirty;
    fn flush_batch(&self, limit: usize) -> FlushBatch<K, V>;
    fn mark_flushed(&self, batch: &FlushBatch<K, V>) -> usize;
    fn len(&self) -> usize;

    #[cfg(any(test, feature = "loom"))]
    fn pending_records(&self) -> Vec<Self::VisibleDirty>;

    #[cfg(any(test, feature = "loom"))]
    fn inflight_records(&self) -> Vec<Self::VisibleDirty>;

    #[cfg(any(test, feature = "loom"))]
    fn next_write(&self) -> WriteId;
}

struct DirtyLog<K, V> {
    next_id: WriteId,
    entries: VecDeque<Arc<DirtyRecord<K, V>>>,
    inflight: Option<FlushBatch<K, V>>,
}

impl<K, V> DirtyLog<K, V> {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            next_id: 0,
            entries: VecDeque::with_capacity(capacity.max(1)),
            inflight: None,
        }
    }
}

pub(crate) struct OrderedFifoDirty<K, V> {
    inner: Mutex<DirtyLog<K, V>>,
}

impl<K, V> DirtyMode<K, V> for OrderedFifoDirty<K, V> {
    type VisibleDirty = Arc<DirtyRecord<K, V>>;

    fn new(capacity: usize) -> Self {
        Self {
            inner: new_mutex(DirtyLog::with_capacity(capacity)),
        }
    }

    fn append(&self, key: K, value: V) -> Self::VisibleDirty {
        let mut inner = lock(&self.inner);
        let id = inner.next_id;
        inner.next_id = inner.next_id.wrapping_add(1);
        let record = Arc::new(DirtyRecord { id, key, value });
        inner.entries.push_back(record.clone());
        record
    }

    fn flush_batch(&self, limit: usize) -> FlushBatch<K, V> {
        let mut inner = lock(&self.inner);
        if let Some(batch) = &inner.inflight {
            return batch.clone();
        }
        let mut entries = Vec::with_capacity(limit);
        while entries.len() < limit {
            let Some(record) = inner.entries.pop_front() else {
                break;
            };
            entries.push(record);
        }
        let batch = FlushBatch::new(entries);
        if !batch.is_empty() {
            inner.inflight = Some(batch.clone());
        }
        batch
    }

    fn mark_flushed(&self, batch: &FlushBatch<K, V>) -> usize {
        let mut inner = lock(&self.inner);
        let Some(inflight) = &inner.inflight else {
            return 0;
        };
        if inflight.len() != batch.len() || inflight.last_id() != batch.last_id() {
            return 0;
        }
        inner
            .inflight
            .take()
            .expect("inflight batch vanished")
            .len()
    }

    fn len(&self) -> usize {
        let inner = lock(&self.inner);
        inner.entries.len() + inner.inflight.as_ref().map_or(0, FlushBatch::len)
    }

    #[cfg(any(test, feature = "loom"))]
    fn pending_records(&self) -> Vec<Self::VisibleDirty> {
        lock(&self.inner).entries.iter().cloned().collect()
    }

    #[cfg(any(test, feature = "loom"))]
    fn inflight_records(&self) -> Vec<Self::VisibleDirty> {
        lock(&self.inner)
            .inflight
            .as_ref()
            .map(|batch| batch.entries.to_vec())
            .unwrap_or_default()
    }

    #[cfg(any(test, feature = "loom"))]
    fn next_write(&self) -> WriteId {
        lock(&self.inner).next_id
    }
}
