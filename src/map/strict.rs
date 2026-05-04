use super::{CacheLogMap, FlushWork, StrictEngine, VisibleValue};
use crate::dirty_mode::DirtyMode;
use crate::entry::{DirtyRecord, EntryState, FlushBatch, PersistBatch, VisibleRef, WriteId};
use crate::sync::{Arc, Ordering, lock};
use ahash::AHashSet;
use scc::hash_map::Entry as MapEntry;
use std::borrow::Borrow;
use std::hash::{BuildHasher, Hash};

impl<K, V, H> CacheLogMap<K, V, H>
where
    K: Clone + Eq + Hash,
    H: BuildHasher + Clone,
{
    pub(super) fn publish_dirty_record(&self, record: Arc<DirtyRecord<K, V>>) {
        let id = record.id;
        if self.is_strict_flushed(id) {
            return;
        }

        let visible_key = record.key.clone();
        let visible = VisibleValue::Dirty(record.clone());
        match self.visible.entry_sync(visible_key.clone()) {
            MapEntry::Occupied(mut occupied) => {
                if matches!(occupied.get(), VisibleValue::Clean(_)) {
                    self.clean_count.fetch_sub(1, Ordering::Relaxed);
                }
                let _ = occupied.insert(visible);
            }
            MapEntry::Vacant(vacant) => {
                vacant.insert_entry(visible);
            }
        }

        if self.is_strict_flushed(id) {
            let _ = self.visible.remove_if_sync(
                &visible_key,
                |entry| matches!(entry, VisibleValue::Dirty(current) if current.id == id),
            );
        }
    }

    pub(super) fn insert_dirty_batch_strict(&self, entries: Vec<(K, V)>) -> Vec<WriteId> {
        let records = self.dirty_mode.append_batch(entries);
        let mut ids = Vec::with_capacity(records.len());
        for record in records {
            ids.push(record.id);
            self.publish_dirty_record(record);
        }
        ids
    }

    pub(super) fn insert_dirty_batch_strict_without_ids(&self, entries: Vec<(K, V)>) -> usize {
        let records = self.dirty_mode.append_batch(entries);
        let len = records.len();
        for record in records {
            self.publish_dirty_record(record);
        }
        len
    }

    pub(super) fn is_strict_flushed(&self, id: WriteId) -> bool {
        let flushed_upto = self.strict_flushed_upto.load(Ordering::SeqCst);
        flushed_upto != u64::MAX && id <= flushed_upto
    }

    pub(super) fn advance_strict_flushed_upto(&self, id: WriteId) {
        let mut current = self.strict_flushed_upto.load(Ordering::SeqCst);
        loop {
            if current != u64::MAX && current >= id {
                return;
            }
            match self.strict_flushed_upto.compare_exchange_weak(
                current,
                id,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return,
                Err(observed) => current = observed,
            }
        }
    }
}

impl<'a, K, V, H> StrictEngine<'a, K, V, H>
where
    K: Clone + Eq + Hash,
    H: BuildHasher + Clone,
{
    pub(crate) fn visible_ref<Q>(&self, key: &Q) -> Option<VisibleRef>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.map
            .visible
            .read_sync(key, |_, visible| visible.visible_ref())
    }

    pub(crate) fn read_full<Q, R>(
        &self,
        key: &Q,
        reader: impl FnOnce(&K, &V, EntryState, VisibleRef) -> R,
    ) -> Option<R>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        let mut reader = Some(reader);
        self.map.visible.read_sync(key, |_, visible| match visible {
            VisibleValue::Dirty(record) => {
                let reader = reader
                    .take()
                    .expect("read_sync closure called more than once");
                reader(
                    &record.key,
                    &record.value,
                    EntryState::Dirty,
                    VisibleRef::Dirty(record.id),
                )
            }
            VisibleValue::Clean(record) => {
                let reader = reader
                    .take()
                    .expect("read_sync closure called more than once");
                reader(
                    &record.key,
                    &record.value,
                    EntryState::Clean,
                    VisibleRef::Clean(record.id),
                )
            }
        })
    }

    pub(crate) fn snapshot_prefix_keys_sorted(&self, prefix: &[u8], limit: usize) -> Vec<K>
    where
        K: Borrow<[u8]>,
    {
        if limit == 0 {
            return Vec::new();
        }
        let _publish = lock(&self.map.coalesced.active_publish);
        let mut keys = Vec::with_capacity(limit.min(1024));
        let mut seen = AHashSet::with_capacity(limit.min(1024));
        self.map.visible.iter_sync(|_, visible| {
            let key = match visible {
                VisibleValue::Dirty(record) => &record.key,
                VisibleValue::Clean(record) => &record.key,
            };
            if key.borrow().starts_with(prefix) && seen.insert((*key).clone()) {
                keys.push((*key).clone());
            }
            true
        });
        keys.sort_unstable_by(|left, right| left.borrow().cmp(right.borrow()));
        if keys.len() > limit {
            keys.truncate(limit);
        }
        keys
    }

    pub(crate) fn insert_dirty(&self, key: K, value: V) -> WriteId {
        let record = self.map.dirty_mode.append(key, value);
        let id = record.id;
        self.map.publish_dirty_record(record);
        id
    }

    pub(crate) fn put(&self, key: K, value: V) {
        let _ = self.insert_dirty(key, value);
    }

    pub(crate) fn insert_dirty_batch(&self, entries: Vec<(K, V)>) -> Vec<WriteId> {
        if entries.is_empty() {
            return Vec::new();
        }
        let _reserved = self.map.visible.reserve(entries.len());
        self.map.insert_dirty_batch_strict(entries)
    }

    pub(crate) fn insert_dirty_batch_without_ids(&self, entries: Vec<(K, V)>) -> usize {
        if entries.is_empty() {
            return 0;
        }
        let _reserved = self.map.visible.reserve(entries.len());
        self.map.insert_dirty_batch_strict_without_ids(entries)
    }

    pub(crate) fn put_batch(&self, entries: Vec<(K, V)>) -> usize {
        self.insert_dirty_batch_without_ids(entries)
    }

    pub(crate) fn flush_batch(&self, limit: usize) -> FlushBatch<K, V> {
        self.map.dirty_mode.flush_batch(limit)
    }

    pub(crate) fn wait_flush_work(&self, limit: usize) -> FlushWork<K, V> {
        match self.map.dirty_mode.wait_for_flush_work(limit) {
            crate::dirty_mode::FlushWork::Batch(batch) => FlushWork::Batch(batch),
            crate::dirty_mode::FlushWork::ForceFlush => FlushWork::ForceFlush,
        }
    }

    pub(crate) fn signal_force_flush(&self) {
        self.map.dirty_mode.signal_force_flush();
    }

    pub(crate) fn mark_flushed(&self, batch: &FlushBatch<K, V>) -> usize {
        let marked = self.map.dirty_mode.mark_flushed(batch);
        if marked == 0 {
            return 0;
        }

        if let Some(last_id) = batch.last_id() {
            self.map.advance_strict_flushed_upto(last_id);
        }
        let strict_flushed_upto = self.map.strict_flushed_upto.load(Ordering::SeqCst);

        for record in batch.iter() {
            let _ = self
                .map
                .visible
                .remove_if_sync(&record.key, |visible| match visible {
                    VisibleValue::Dirty(current) => current.id <= strict_flushed_upto,
                    VisibleValue::Clean(_) => true,
                });
        }
        marked
    }

    pub(crate) fn with_flush_batch<E>(
        &self,
        limit: usize,
        persist: impl FnOnce(&PersistBatch<K, V>) -> Result<(), E>,
    ) -> Result<usize, E> {
        let batch = self.flush_batch(limit.max(1));
        if batch.is_empty() {
            return Ok(0);
        }
        let persist_batch = PersistBatch::from_flush_batch(batch.clone());
        persist(&persist_batch)?;
        Ok(self.mark_flushed(&batch))
    }

    pub(crate) fn flush_now<E>(
        &self,
        limit: usize,
        mut persist: impl FnMut(&PersistBatch<K, V>) -> Result<(), E>,
    ) -> Result<usize, E> {
        let mut total = 0_usize;
        loop {
            let batch = self.flush_batch(limit.max(1));
            if batch.is_empty() {
                return Ok(total);
            }
            let persist_batch = PersistBatch::from_flush_batch(batch.clone());
            persist(&persist_batch)?;
            total += self.mark_flushed(&batch);
        }
    }

    pub(crate) fn with_persisted_scan<R, E>(
        &self,
        flush_limit: usize,
        persist: impl FnMut(&PersistBatch<K, V>) -> Result<(), E>,
        scan: impl FnOnce() -> Result<R, E>,
    ) -> Result<R, E> {
        let _ = self.flush_now(flush_limit, persist)?;
        scan()
    }
}
