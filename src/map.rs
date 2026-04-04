use std::collections::VecDeque;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hash};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use arc_swap::ArcSwapOption;
use crossbeam_queue::ArrayQueue;
use parking_lot::Mutex;
use scc::HashIndex as ConcurrentHashIndex;
use scc::hash_index::Entry as IndexEntry;

use crate::entry::{
    CacheId, CleanRecord, DirtyRecord, EntryState, FlushBatch, VisibleRef, WriteId,
};

#[derive(Debug)]
enum VisibleRecord<K, V> {
    Dirty(Arc<DirtyRecord<K, V>>),
    Clean(Arc<CleanRecord<K, V>>),
}

struct VisibleSlot<K, V> {
    current: ArcSwapOption<VisibleRecord<K, V>>,
}

impl<K, V> VisibleRecord<K, V> {
    fn visible_ref(&self) -> VisibleRef {
        match self {
            Self::Dirty(record) => VisibleRef::Dirty(record.id),
            Self::Clean(record) => VisibleRef::Clean(record.id),
        }
    }
}

impl<K, V> VisibleSlot<K, V> {
    fn with_record(record: Arc<VisibleRecord<K, V>>) -> Self {
        Self {
            current: ArcSwapOption::from(Some(record)),
        }
    }
}

struct DirtyLog<K, V> {
    entries: VecDeque<Arc<DirtyRecord<K, V>>>,
    inflight: Option<FlushBatch<K, V>>,
}

impl<K, V> DirtyLog<K, V> {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(capacity),
            inflight: None,
        }
    }

    fn append(&mut self, record: Arc<DirtyRecord<K, V>>) {
        self.entries.push_back(record);
    }

    fn pending_batch(&mut self, limit: usize) -> FlushBatch<K, V> {
        if let Some(batch) = &self.inflight {
            return batch.clone();
        }
        let mut entries = Vec::with_capacity(limit);
        while entries.len() < limit {
            let Some(record) = self.entries.pop_front() else {
                break;
            };
            entries.push(record);
        }
        let batch = FlushBatch::new(entries);
        if !batch.is_empty() {
            self.inflight = Some(batch.clone());
        }
        batch
    }

    fn mark_flushed(&mut self, batch: &FlushBatch<K, V>) -> usize {
        let Some(inflight) = &self.inflight else {
            return 0;
        };
        if inflight.len() != batch.len() || inflight.last_id() != batch.last_id() {
            return 0;
        }
        let flushed = self.inflight.take().expect("inflight batch vanished");
        flushed.len()
    }

    fn len(&self) -> usize {
        self.entries.len() + self.inflight.as_ref().map_or(0, FlushBatch::len)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CacheLogConfig {
    pub visible_capacity: usize,
    pub dirty_log_capacity: usize,
    pub clean_capacity: usize,
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
        }
    }
}

impl Default for CacheLogConfig {
    fn default() -> Self {
        Self::new(1024, 1024, 1024)
    }
}

pub struct CacheLogMap<K, V, H = RandomState>
where
    K: Clone + Eq + Hash,
    H: BuildHasher + Clone,
{
    visible: ConcurrentHashIndex<K, VisibleSlot<K, V>, H>,
    dirty_log: Mutex<DirtyLog<K, V>>,
    clean_fifo: ArrayQueue<(K, CacheId)>,
    clean_count: AtomicUsize,
    next_write: AtomicU64,
    next_cache: AtomicU64,
}

impl<K, V> CacheLogMap<K, V, RandomState>
where
    K: Clone + Eq + Hash,
{
    pub fn new(config: CacheLogConfig) -> Self {
        Self::with_hasher(config, RandomState::new())
    }
}

impl<K, V, H> CacheLogMap<K, V, H>
where
    K: Clone + Eq + Hash,
    H: BuildHasher + Clone,
{
    pub fn with_hasher(config: CacheLogConfig, build_hasher: H) -> Self {
        Self {
            visible: ConcurrentHashIndex::with_capacity_and_hasher(
                config.visible_capacity,
                build_hasher,
            ),
            dirty_log: Mutex::new(DirtyLog::with_capacity(config.dirty_log_capacity)),
            clean_fifo: ArrayQueue::new(config.clean_capacity.max(1)),
            clean_count: AtomicUsize::new(0),
            next_write: AtomicU64::new(0),
            next_cache: AtomicU64::new(0),
        }
    }

    pub fn visible_len(&self) -> usize {
        self.visible.len()
    }

    pub fn dirty_log_len(&self) -> usize {
        self.dirty_log.lock().len()
    }

    pub fn clean_store_len(&self) -> usize {
        self.clean_count.load(Ordering::Relaxed)
    }

    pub fn contains(&self, key: &K) -> bool {
        self.read(key, |_, _, _, _| ()).is_some()
    }

    pub fn visible_ref(&self, key: &K) -> Option<VisibleRef> {
        self.visible
            .peek_with(key, |_, slot| {
                let current = slot.current.load();
                current.as_ref().map(|record| record.visible_ref())
            })
            .flatten()
    }

    pub fn get_cloned(&self, key: &K) -> Option<(V, EntryState, VisibleRef)>
    where
        V: Clone,
    {
        self.read(key, |_, value, state, visible| {
            (value.clone(), state, visible)
        })
    }

    pub fn read<R>(
        &self,
        key: &K,
        reader: impl FnOnce(&K, &V, EntryState, VisibleRef) -> R,
    ) -> Option<R> {
        let mut reader = Some(reader);
        self.visible.peek_with(key, |_, slot| {
            let current = slot.current.load();
            current.as_ref().map(|current| match current.as_ref() {
                VisibleRecord::Dirty(record) => {
                    let reader = reader
                        .take()
                        .expect("peek_with closure called more than once");
                    reader(
                        &record.key,
                        &record.value,
                        EntryState::Dirty,
                        VisibleRef::Dirty(record.id),
                    )
                }
                VisibleRecord::Clean(record) => {
                    let reader = reader
                        .take()
                        .expect("peek_with closure called more than once");
                    reader(
                        &record.key,
                        &record.value,
                        EntryState::Clean,
                        VisibleRef::Clean(record.id),
                    )
                }
            })
        })?
    }

    pub fn insert_dirty(&self, key: K, value: V) -> WriteId {
        let id = self.next_write.fetch_add(1, Ordering::Relaxed);
        let record = Arc::new(DirtyRecord { id, key, value });
        self.dirty_log.lock().append(record.clone());
        let visible_key = record.key.clone();
        let visible = Arc::new(VisibleRecord::Dirty(record));
        match self.visible.entry_sync(visible_key) {
            IndexEntry::Occupied(occupied) => {
                let current = occupied.get().current.load();
                if current.as_ref().is_some_and(|record| matches!(record.as_ref(), VisibleRecord::Clean(_))) {
                    self.clean_count.fetch_sub(1, Ordering::Relaxed);
                }
                occupied.get().current.store(Some(visible));
            }
            IndexEntry::Vacant(vacant) => {
                vacant.insert_entry(VisibleSlot::with_record(visible));
            }
        }
        id
    }

    pub fn upsert_dirty(&self, key: K, value: V) -> WriteId {
        self.insert_dirty(key, value)
    }

    pub fn insert_clean_if_absent(&self, key: K, value: V) -> Option<CacheId> {
        let id = self.next_cache.fetch_add(1, Ordering::Relaxed);
        let record = Arc::new(CleanRecord { id, key, value });
        let visible = Arc::new(VisibleRecord::Clean(record.clone()));
        let inserted = match self.visible.entry_sync(record.key.clone()) {
            IndexEntry::Vacant(vacant) => {
                vacant.insert_entry(VisibleSlot::with_record(visible));
                self.clean_count.fetch_add(1, Ordering::Relaxed);
                true
            }
            IndexEntry::Occupied(occupied) => {
                let can_replace = occupied.get().current.load().is_none();
                if can_replace {
                    occupied.get().current.store(Some(visible));
                    self.clean_count.fetch_add(1, Ordering::Relaxed);
                    true
                } else {
                    false
                }
            }
        };
        if inserted {
            self.enqueue_clean(record.key.clone(), id);
            Some(id)
        } else {
            None
        }
    }

    pub fn evict_clean(&self, key: &K) -> bool {
        self.visible.peek_with(key, |_, slot| {
            let current = slot.current.load();
            if current.as_ref().is_some_and(|record| matches!(record.as_ref(), VisibleRecord::Clean(_))) {
                slot.current.store(None);
                self.clean_count.fetch_sub(1, Ordering::Relaxed);
                true
            } else {
                false
            }
        }).unwrap_or(false)
    }

    pub fn cleanup_stale_visible(&self, key: &K) -> bool {
        let Some(visible_ref) = self.visible_ref(key) else {
            return false;
        };
        self.cleanup_stale_visible_matching(key, visible_ref)
    }

    pub fn flush_batch(&self, limit: usize) -> FlushBatch<K, V> {
        self.dirty_log.lock().pending_batch(limit)
    }

    pub fn mark_flushed(&self, batch: &FlushBatch<K, V>) -> usize {
        let flushed_records = batch
            .iter()
            .map(|entry| (entry.key.clone(), entry.id))
            .collect::<Vec<_>>();
        let mut dirty_log = self.dirty_log.lock();
        let marked = dirty_log.mark_flushed(batch);
        drop(dirty_log);
        if marked == 0 {
            return 0;
        }
        for (key, id) in flushed_records {
            let _ = self.visible.peek_with(&key, |_, slot| {
                let current = slot.current.load();
                if current.as_ref().is_some_and(|record| {
                    matches!(record.as_ref(), VisibleRecord::Dirty(record) if record.id == id)
                }) {
                    slot.current.store(None);
                }
            });
        }
        marked
    }

    fn cleanup_stale_visible_matching(&self, key: &K, expected: VisibleRef) -> bool {
        self.visible.remove_if_sync(key, |slot| {
            slot.current
                .load()
                .as_ref()
                .is_none_or(|record| record.visible_ref() == expected)
        })
    }

    fn enqueue_clean(&self, key: K, id: CacheId) {
        let mut pending = (key, id);
        loop {
            match self.clean_fifo.push(pending) {
                Ok(()) => break,
                Err(returned) => {
                    pending = returned;
                    if let Some((old_key, old_id)) = self.clean_fifo.pop() {
                        let _ = self.visible.peek_with(&old_key, |_, slot| {
                            let current = slot.current.load();
                            if current.as_ref().is_some_and(|record| {
                                matches!(record.as_ref(), VisibleRecord::Clean(record) if record.id == old_id)
                            }) {
                                slot.current.store(None);
                                self.clean_count.fetch_sub(1, Ordering::Relaxed);
                            }
                        });
                    }
                }
            }
        }
    }
}
