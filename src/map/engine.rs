use super::{CacheLogMap, FlushWork, VisibleValue};
#[cfg(not(feature = "loom"))]
use crate::background_flush::BackgroundFlushService;
use crate::entry::{EntryState, FlushBatch, PersistBatch, VisibleRef, WriteId};
#[cfg(not(feature = "loom"))]
use crate::sync::Arc;
use crate::sync::lock;
use ahash::AHashSet;
use std::borrow::Borrow;
use std::hash::{BuildHasher, Hash};

pub(crate) struct StrictEngine<'a, K, V, H>
where
    K: Clone + Eq + Hash,
    H: BuildHasher + Clone,
{
    pub(crate) map: &'a CacheLogMap<K, V, H>,
}

pub(crate) struct CoalescedEngine<'a, K, V, H>
where
    K: Clone + Eq + Hash,
    H: BuildHasher + Clone,
{
    pub(crate) map: &'a CacheLogMap<K, V, H>,
}

#[cfg(not(feature = "loom"))]
pub(crate) struct StrictArcEngine<K, V, H>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    V: Send + Sync + 'static,
    H: BuildHasher + Clone + Send + Sync + 'static,
{
    pub(crate) map: Arc<CacheLogMap<K, V, H>>,
}

#[cfg(not(feature = "loom"))]
pub(crate) struct CoalescedArcEngine<K, V, H>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    V: Send + Sync + 'static,
    H: BuildHasher + Clone + Send + Sync + 'static,
{
    pub(crate) map: Arc<CacheLogMap<K, V, H>>,
}

impl<'a, K, V, H> CoalescedEngine<'a, K, V, H>
where
    K: Clone + Eq + Hash,
    H: BuildHasher + Clone,
{
    pub(crate) fn visible_ref<Q>(&self, key: &Q) -> Option<VisibleRef>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        let _publish = lock(&self.map.coalesced.active_publish);
        self.map
            .coalesced
            .read_visible_ref(key)
            .or_else(|| {
                self.map
                    .visible
                    .read_sync(key, |_, visible| visible.visible_ref())
            })
            .or_else(|| {
                self.map
                    .read_coalesced_inflight(key, |record| VisibleRef::Dirty(record.id))
            })
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
        let _publish = lock(&self.map.coalesced.active_publish);
        let mut reader = Some(reader);
        if let Some(result) = self.map.coalesced.read_active(key, |stored_key, record| {
            let reader = reader
                .take()
                .expect("read_sync closure called more than once");
            reader(
                stored_key,
                &record.value,
                EntryState::Dirty,
                VisibleRef::Dirty(record.id),
            )
        }) {
            return Some(result);
        }

        if let Some(result) = self.map.read_coalesced_inflight(key, |record| {
            let reader = reader
                .take()
                .expect("read_coalesced_inflight called more than once");
            reader(
                &record.key,
                &record.value,
                EntryState::Dirty,
                VisibleRef::Dirty(record.id),
            )
        }) {
            return Some(result);
        }

        if let Some(result) = self.map.coalesced.read_draining(key, |stored_key, record| {
            let reader = reader
                .take()
                .expect("read_draining closure called more than once");
            reader(
                stored_key,
                &record.value,
                EntryState::Dirty,
                VisibleRef::Dirty(record.id),
            )
        }) {
            return Some(result);
        }

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
        let mut keys = Vec::with_capacity(limit.min(1024));
        let mut seen = AHashSet::with_capacity(limit.min(1024));
        self.map.coalesced.for_each_active_key(|key| {
            if key.borrow().starts_with(prefix) && seen.insert(key.clone()) {
                keys.push(key.clone());
            }
        });
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
        self.map.coalesced.with_inflight(|inflight| {
            if let Some(inflight) = inflight {
                for record in inflight.batch.iter() {
                    if record.key.borrow().starts_with(prefix) && seen.insert(record.key.clone()) {
                        keys.push(record.key.clone());
                    }
                }
            }
        });
        self.map.coalesced.for_each_draining_key(|key| {
            if key.borrow().starts_with(prefix) && seen.insert(key.clone()) {
                keys.push(key.clone());
            }
        });
        keys.sort_unstable_by(|left, right| left.borrow().cmp(right.borrow()));
        if keys.len() > limit {
            keys.truncate(limit);
        }
        keys
    }

    pub(crate) fn insert_dirty(&self, key: K, value: V) -> WriteId {
        self.map.publish_coalesced_single_write(key, value)
    }

    pub(crate) fn put(&self, key: K, value: V) {
        self.map
            .publish_coalesced_single_write_without_id(key, value);
    }

    pub(crate) fn insert_dirty_batch(&self, entries: Vec<(K, V)>) -> Vec<WriteId> {
        if entries.is_empty() {
            return Vec::new();
        }
        self.map.insert_dirty_batch_coalesced(entries)
    }

    pub(crate) fn insert_dirty_batch_without_ids(&self, entries: Vec<(K, V)>) -> usize {
        if entries.is_empty() {
            return 0;
        }
        self.map.insert_dirty_batch_coalesced_without_ids(entries)
    }

    pub(crate) fn put_batch(&self, entries: Vec<(K, V)>) -> usize {
        self.insert_dirty_batch_without_ids(entries)
    }

    pub(crate) fn flush_batch(&self, limit: usize) -> FlushBatch<K, V> {
        self.map.flush_batch_coalesced(limit)
    }

    pub(crate) fn wait_flush_work(&self, limit: usize) -> FlushWork<K, V> {
        let batch = self.map.flush_batch_coalesced(limit);
        if batch.is_empty() {
            FlushWork::ForceFlush
        } else {
            FlushWork::Batch(batch)
        }
    }

    pub(crate) fn signal_force_flush(&self) {}

    pub(crate) fn mark_flushed(&self, batch: &FlushBatch<K, V>) -> usize {
        self.map.mark_flushed_coalesced(batch)
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

#[cfg(not(feature = "loom"))]
impl<K, V, H> StrictArcEngine<K, V, H>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    V: Send + Sync + 'static,
    H: BuildHasher + Clone + Send + Sync + 'static,
{
    pub(crate) fn background_flush_service(
        &self,
        persist: impl Fn(&PersistBatch<K, V>) -> Result<(), String> + Send + Sync + 'static,
    ) -> BackgroundFlushService {
        let persist = Arc::new(persist);
        let dirty_len_map = self.map.clone();
        let dirty_len: Arc<dyn Fn() -> usize + Send + Sync> =
            Arc::new(move || dirty_len_map.dirty_log_len());
        let flush_map = self.map.clone();
        let flush: Arc<dyn Fn(usize) -> Result<(), String> + Send + Sync> =
            Arc::new(move |limit| {
                flush_map
                    .strict_engine()
                    .flush_now(limit, |batch| persist(batch))
                    .map(|_| ())
            });
        BackgroundFlushService { dirty_len, flush }
    }
}

#[cfg(not(feature = "loom"))]
impl<K, V, H> CoalescedArcEngine<K, V, H>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    V: Send + Sync + 'static,
    H: BuildHasher + Clone + Send + Sync + 'static,
{
    pub(crate) fn background_flush_service(
        &self,
        persist: impl Fn(&PersistBatch<K, V>) -> Result<(), String> + Send + Sync + 'static,
    ) -> BackgroundFlushService {
        let persist = Arc::new(persist);
        let dirty_len_map = self.map.clone();
        let dirty_len: Arc<dyn Fn() -> usize + Send + Sync> =
            Arc::new(move || dirty_len_map.dirty_log_len());
        let flush_map = self.map.clone();
        let flush: Arc<dyn Fn(usize) -> Result<(), String> + Send + Sync> =
            Arc::new(move |limit| {
                flush_map
                    .coalesced_engine()
                    .flush_now(limit, |batch| persist(batch))
                    .map(|_| ())
            });
        BackgroundFlushService { dirty_len, flush }
    }
}
