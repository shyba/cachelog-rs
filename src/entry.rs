use crate::sync::Arc;
use std::slice;

pub type WriteId = u64;
pub type CacheId = u64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntryState {
    Dirty,
    Clean,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VisibleRef {
    Dirty(WriteId),
    Clean(CacheId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirtyRecord<K, V> {
    pub id: WriteId,
    pub key: K,
    pub value: V,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CleanRecord<K, V> {
    pub id: CacheId,
    pub key: K,
    pub value: V,
}

#[derive(Debug)]
enum FlushBatchEntries<K, V> {
    Shared(Arc<Vec<Arc<DirtyRecord<K, V>>>>),
    Owned(Arc<Vec<DirtyRecord<K, V>>>),
}

#[derive(Debug)]
pub struct FlushBatch<K, V> {
    entries: FlushBatchEntries<K, V>,
}

#[derive(Clone, Debug)]
pub struct PersistBatch<K, V> {
    inner: FlushBatch<K, V>,
}

impl<K, V> Clone for FlushBatch<K, V> {
    fn clone(&self) -> Self {
        Self {
            entries: match &self.entries {
                FlushBatchEntries::Shared(entries) => FlushBatchEntries::Shared(entries.clone()),
                FlushBatchEntries::Owned(entries) => FlushBatchEntries::Owned(entries.clone()),
            },
        }
    }
}

impl<K, V> FlushBatch<K, V> {
    pub(crate) fn new(entries: Vec<Arc<DirtyRecord<K, V>>>) -> Self {
        Self {
            entries: FlushBatchEntries::Shared(Arc::new(entries)),
        }
    }

    pub(crate) fn new_owned(entries: Vec<DirtyRecord<K, V>>) -> Self {
        Self {
            entries: FlushBatchEntries::Owned(Arc::new(entries)),
        }
    }

    pub fn is_empty(&self) -> bool {
        match &self.entries {
            FlushBatchEntries::Shared(entries) => entries.is_empty(),
            FlushBatchEntries::Owned(entries) => entries.is_empty(),
        }
    }

    pub fn len(&self) -> usize {
        match &self.entries {
            FlushBatchEntries::Shared(entries) => entries.len(),
            FlushBatchEntries::Owned(entries) => entries.len(),
        }
    }

    pub fn last_id(&self) -> Option<WriteId> {
        match &self.entries {
            FlushBatchEntries::Shared(entries) => entries.last().map(|entry| entry.id),
            FlushBatchEntries::Owned(entries) => entries.last().map(|entry| entry.id),
        }
    }

    pub fn iter(&self) -> FlushBatchIter<'_, K, V> {
        match &self.entries {
            FlushBatchEntries::Shared(entries) => FlushBatchIter::Shared(entries.iter()),
            FlushBatchEntries::Owned(entries) => FlushBatchIter::Owned(entries.iter()),
        }
    }

    pub(crate) fn get(&self, index: usize) -> Option<&DirtyRecord<K, V>> {
        match &self.entries {
            FlushBatchEntries::Shared(entries) => entries.get(index).map(AsRef::as_ref),
            FlushBatchEntries::Owned(entries) => entries.get(index),
        }
    }

    pub(crate) fn ptr_eq(&self, other: &Self) -> bool {
        match (&self.entries, &other.entries) {
            (FlushBatchEntries::Shared(left), FlushBatchEntries::Shared(right)) => {
                Arc::ptr_eq(left, right)
            }
            (FlushBatchEntries::Owned(left), FlushBatchEntries::Owned(right)) => {
                Arc::ptr_eq(left, right)
            }
            _ => false,
        }
    }

    #[cfg(any(test, feature = "loom"))]
    pub(crate) fn cloned_arc_entries(&self) -> Vec<Arc<DirtyRecord<K, V>>>
    where
        K: Clone,
        V: Clone,
    {
        match &self.entries {
            FlushBatchEntries::Shared(entries) => entries.as_ref().clone(),
            FlushBatchEntries::Owned(entries) => entries.iter().cloned().map(Arc::new).collect(),
        }
    }
}

impl<K, V> PersistBatch<K, V> {
    pub(crate) fn from_flush_batch(inner: FlushBatch<K, V>) -> Self {
        Self { inner }
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn iter(&self) -> PersistBatchIter<'_, K, V> {
        PersistBatchIter {
            inner: self.inner.iter(),
        }
    }
}

pub enum FlushBatchIter<'a, K, V> {
    Shared(slice::Iter<'a, Arc<DirtyRecord<K, V>>>),
    Owned(slice::Iter<'a, DirtyRecord<K, V>>),
}

impl<'a, K, V> Iterator for FlushBatchIter<'a, K, V> {
    type Item = &'a DirtyRecord<K, V>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Shared(entries) => entries.next().map(AsRef::as_ref),
            Self::Owned(entries) => entries.next(),
        }
    }
}

pub struct PersistBatchEntry<'a, K, V> {
    record: &'a DirtyRecord<K, V>,
}

impl<'a, K, V> PersistBatchEntry<'a, K, V> {
    pub fn key(&self) -> &'a K {
        &self.record.key
    }

    pub fn value(&self) -> &'a V {
        &self.record.value
    }
}

pub struct PersistBatchIter<'a, K, V> {
    inner: FlushBatchIter<'a, K, V>,
}

impl<'a, K, V> Iterator for PersistBatchIter<'a, K, V> {
    type Item = PersistBatchEntry<'a, K, V>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|record| PersistBatchEntry { record })
    }
}
