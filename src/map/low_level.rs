use std::borrow::Borrow;
use std::hash::{BuildHasher, Hash};

use super::LowLevelMap;
use super::types::FlushWork;
use crate::entry::{EntryState, FlushBatch, VisibleRef, WriteId};
#[cfg(any(test, feature = "dev-tools"))]
use crate::sync::Arc;

impl<K, V, H> LowLevelMap<'_, K, V, H>
where
    K: Clone + Eq + Hash,
    H: BuildHasher + Clone,
{
    pub fn visible_ref<Q>(&self, key: &Q) -> Option<VisibleRef>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.map.visible_ref(key)
    }

    pub fn get_cloned_full<Q>(&self, key: &Q) -> Option<(V, EntryState, VisibleRef)>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
        V: Clone,
    {
        self.map.get_cloned_full(key)
    }

    pub fn read_full<Q, R>(
        &self,
        key: &Q,
        reader: impl FnOnce(&K, &V, EntryState, VisibleRef) -> R,
    ) -> Option<R>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.map.read_full(key, reader)
    }

    pub fn insert_dirty(&self, key: K, value: V) -> WriteId {
        self.map.insert_dirty(key, value)
    }

    pub fn insert_dirty_batch(&self, entries: Vec<(K, V)>) -> Vec<WriteId> {
        self.map.insert_dirty_batch(entries)
    }

    pub fn insert_dirty_batch_without_ids(&self, entries: Vec<(K, V)>) -> usize {
        self.map.insert_dirty_batch_without_ids(entries)
    }

    pub fn upsert_dirty(&self, key: K, value: V) -> WriteId {
        self.map.upsert_dirty(key, value)
    }

    pub fn flush_batch(&self, limit: usize) -> FlushBatch<K, V> {
        self.map.flush_batch(limit)
    }

    pub fn wait_flush_work(&self, limit: usize) -> FlushWork<K, V> {
        self.map.wait_flush_work(limit)
    }

    pub fn signal_force_flush(&self) {
        self.map.signal_force_flush();
    }

    pub fn mark_flushed(&self, batch: &FlushBatch<K, V>) -> usize {
        self.map.mark_flushed(batch)
    }
}

impl<H> LowLevelMap<'_, Vec<u8>, Vec<u8>, H>
where
    H: BuildHasher + Clone,
{
    pub fn insert_dirty_borrowed(&self, key: &[u8], value: &[u8]) -> WriteId {
        self.map.insert_dirty_borrowed(key, value)
    }

    pub fn insert_dirty_batch_borrowed_without_ids<'a, I>(&self, entries: I) -> usize
    where
        I: IntoIterator<Item = (&'a [u8], &'a [u8])>,
    {
        self.map.insert_dirty_batch_borrowed_without_ids(entries)
    }
}

#[cfg(any(test, feature = "dev-tools"))]
impl<H> LowLevelMap<'_, Vec<u8>, Arc<Vec<u8>>, H>
where
    H: BuildHasher + Clone,
{
    pub fn insert_dirty_borrowed(&self, key: &[u8], value: &[u8]) -> WriteId {
        self.map.insert_dirty_borrowed(key, value)
    }

    pub fn insert_dirty_batch_borrowed_without_ids<'a, I>(&self, entries: I) -> usize
    where
        I: IntoIterator<Item = (&'a [u8], &'a [u8])>,
    {
        self.map.insert_dirty_batch_borrowed_without_ids(entries)
    }
}

#[cfg(all(not(feature = "loom"), any(test, feature = "dev-tools")))]
impl<H> LowLevelMap<'_, Vec<u8>, Arc<[u8]>, H>
where
    H: BuildHasher + Clone,
{
    pub fn insert_dirty_borrowed(&self, key: &[u8], value: &[u8]) -> WriteId {
        self.map.insert_dirty_borrowed(key, value)
    }

    pub fn insert_dirty_preowned(&self, key: Vec<u8>, value: Arc<[u8]>) -> WriteId {
        self.map.insert_dirty_preowned(key, value)
    }

    pub fn insert_dirty_batch_preowned_without_ids(
        &self,
        entries: Vec<(Vec<u8>, Arc<[u8]>)>,
    ) -> usize {
        self.map.insert_dirty_batch_preowned_without_ids(entries)
    }

    pub fn insert_dirty_batch_borrowed_without_ids<'a, I>(&self, entries: I) -> usize
    where
        I: IntoIterator<Item = (&'a [u8], &'a [u8])>,
    {
        self.map.insert_dirty_batch_borrowed_without_ids(entries)
    }
}

#[cfg(any(test, feature = "dev-tools"))]
impl<H> LowLevelMap<'_, Vec<u8>, bytes::Bytes, H>
where
    H: BuildHasher + Clone,
{
    pub fn insert_dirty_borrowed(&self, key: &[u8], value: &[u8]) -> WriteId {
        self.map.insert_dirty_borrowed(key, value)
    }

    pub fn insert_dirty_owned_key_borrowed_value(&self, key: Vec<u8>, value: &[u8]) -> WriteId {
        self.map.insert_dirty_owned_key_borrowed_value(key, value)
    }

    pub fn insert_dirty_preowned(&self, key: Vec<u8>, value: bytes::Bytes) -> WriteId {
        self.map.insert_dirty_preowned(key, value)
    }

    pub fn insert_dirty_batch_owned_keys_borrowed_values_without_ids<'a, I>(
        &self,
        entries: I,
    ) -> usize
    where
        I: IntoIterator<Item = (Vec<u8>, &'a [u8])>,
    {
        self.map
            .insert_dirty_batch_owned_keys_borrowed_values_without_ids(entries)
    }

    pub fn insert_dirty_batch_preowned_without_ids(
        &self,
        entries: Vec<(Vec<u8>, bytes::Bytes)>,
    ) -> usize {
        self.map.insert_dirty_batch_preowned_without_ids(entries)
    }

    pub fn insert_dirty_batch_borrowed_without_ids<'a, I>(&self, entries: I) -> usize
    where
        I: IntoIterator<Item = (&'a [u8], &'a [u8])>,
    {
        self.map.insert_dirty_batch_borrowed_without_ids(entries)
    }
}
