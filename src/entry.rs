use std::sync::Arc;

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
pub struct FlushBatch<K, V> {
    pub(crate) entries: Vec<Arc<DirtyRecord<K, V>>>,
}

impl<K, V> Clone for FlushBatch<K, V> {
    fn clone(&self) -> Self {
        Self {
            entries: self.entries.clone(),
        }
    }
}

impl<K, V> FlushBatch<K, V> {
    pub(crate) fn new(entries: Vec<Arc<DirtyRecord<K, V>>>) -> Self {
        Self { entries }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn last_id(&self) -> Option<WriteId> {
        self.entries.last().map(|entry| entry.id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &DirtyRecord<K, V>> {
        self.entries.iter().map(AsRef::as_ref)
    }
}
