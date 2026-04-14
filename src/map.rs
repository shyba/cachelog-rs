use ahash::{AHashMap, AHashSet};
use std::borrow::Borrow;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hash};

use scc::HashMap as ConcurrentHashMap;
use scc::hash_map::Entry as MapEntry;

use crate::dirty_mode::{DirtyAllocMode, DirtyMode, DirtyQueueBackend, OrderedFifoDirty};
use crate::entry::{
    CacheId, CleanRecord, DirtyRecord, EntryState, FlushBatch, VisibleRef, WriteId,
};
use crate::sync::{Arc, AtomicU64, AtomicUsize, BoundedQueue, Mutex, Ordering, lock, new_mutex};

#[derive(Debug, Clone)]
enum VisibleValue<K, V> {
    Dirty(Arc<DirtyRecord<K, V>>),
    Clean(Arc<CleanRecord<K, V>>),
}

impl<K, V> VisibleValue<K, V> {
    fn visible_ref(&self) -> VisibleRef {
        match self {
            Self::Dirty(record) => VisibleRef::Dirty(record.id),
            Self::Clean(record) => VisibleRef::Clean(record.id),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CacheLogConfig {
    pub visible_capacity: usize,
    pub dirty_log_capacity: usize,
    pub clean_capacity: usize,
    pub dirty_write_mode: DirtyWriteMode,
    pub dirty_alloc_mode: DirtyAllocMode,
    pub dirty_queue_backend: DirtyQueueBackend,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum DirtyWriteMode {
    #[default]
    StrictLog,
    CoalescedMap,
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
            dirty_alloc_mode: DirtyAllocMode::ChunkedArena,
            dirty_queue_backend: DirtyQueueBackend::Kanal,
        }
    }

    pub const fn with_dirty_write_mode(mut self, mode: DirtyWriteMode) -> Self {
        self.dirty_write_mode = mode;
        self
    }

    pub const fn with_dirty_alloc_mode(mut self, mode: DirtyAllocMode) -> Self {
        self.dirty_alloc_mode = mode;
        self
    }

    pub const fn with_dirty_queue_backend(mut self, backend: DirtyQueueBackend) -> Self {
        self.dirty_queue_backend = backend;
        self
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
    visible: ConcurrentHashMap<K, VisibleValue<K, V>, H>,
    dirty_mode: OrderedFifoDirty<K, V>,
    dirty_write_mode: DirtyWriteMode,
    clean_fifo: BoundedQueue<(K, CacheId)>,
    clean_count: AtomicUsize,
    next_cache: AtomicU64,
    next_dirty_write: AtomicU64,
    coalesced_dirty_count: AtomicUsize,
    coalesced_inflight: Mutex<Option<FlushBatch<K, V>>>,
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
            visible: ConcurrentHashMap::with_capacity_and_hasher(
                config.visible_capacity,
                build_hasher.clone(),
            ),
            dirty_mode: OrderedFifoDirty::new(
                config.dirty_log_capacity,
                config.dirty_queue_backend,
                config.dirty_alloc_mode,
            ),
            dirty_write_mode: config.dirty_write_mode,
            clean_fifo: BoundedQueue::new(config.clean_capacity),
            clean_count: AtomicUsize::new(0),
            next_cache: AtomicU64::new(0),
            next_dirty_write: AtomicU64::new(0),
            coalesced_dirty_count: AtomicUsize::new(0),
            coalesced_inflight: new_mutex(None),
        }
    }

    pub fn visible_len(&self) -> usize {
        self.visible.len()
    }

    pub fn dirty_log_len(&self) -> usize {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => self.dirty_mode.len(),
            DirtyWriteMode::CoalescedMap => self.coalesced_dirty_count.load(Ordering::Relaxed),
        }
    }

    pub fn clean_store_len(&self) -> usize {
        self.clean_count.load(Ordering::Relaxed)
    }

    pub fn contains<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.read(key, |_, _, _, _| ()).is_some()
    }

    pub fn visible_ref<Q>(&self, key: &Q) -> Option<VisibleRef>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.visible
            .read_sync(key, |_, visible| visible.visible_ref())
    }

    pub fn get_cloned<Q>(&self, key: &Q) -> Option<(V, EntryState, VisibleRef)>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
        V: Clone,
    {
        self.read(key, |_, value, state, visible| {
            (value.clone(), state, visible)
        })
    }

    pub fn read<Q, R>(
        &self,
        key: &Q,
        reader: impl FnOnce(&K, &V, EntryState, VisibleRef) -> R,
    ) -> Option<R>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        let mut reader = Some(reader);
        self.visible.read_sync(key, |_, visible| match visible {
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

    pub fn for_each_prefix(
        &self,
        prefix: &[u8],
        mut reader: impl FnMut(&K, &V, EntryState, VisibleRef),
        limit: usize,
    ) -> usize
    where
        K: Borrow<[u8]>,
    {
        if limit == 0 {
            return 0;
        }
        let mut matched = 0_usize;
        self.visible.iter_sync(|_, visible| {
            if matched >= limit {
                return false;
            }
            match visible {
                VisibleValue::Dirty(record) => {
                    if record.key.borrow().starts_with(prefix) {
                        reader(
                            &record.key,
                            &record.value,
                            EntryState::Dirty,
                            VisibleRef::Dirty(record.id),
                        );
                        matched += 1;
                    }
                }
                VisibleValue::Clean(record) => {
                    if record.key.borrow().starts_with(prefix) {
                        reader(
                            &record.key,
                            &record.value,
                            EntryState::Clean,
                            VisibleRef::Clean(record.id),
                        );
                        matched += 1;
                    }
                }
            }
            matched < limit
        });
        matched
    }

    pub fn for_each_prefix_key(
        &self,
        prefix: &[u8],
        mut reader: impl FnMut(&K),
        limit: usize,
    ) -> usize
    where
        K: Borrow<[u8]>,
    {
        if limit == 0 {
            return 0;
        }
        let mut matched = 0_usize;
        self.visible.iter_sync(|_, visible| {
            if matched >= limit {
                return false;
            }
            let key = match visible {
                VisibleValue::Dirty(record) => &record.key,
                VisibleValue::Clean(record) => &record.key,
            };
            if key.borrow().starts_with(prefix) {
                reader(key);
                matched += 1;
            }
            matched < limit
        });
        matched
    }

    pub fn list_prefix<R>(
        &self,
        prefix: &[u8],
        mut reader: impl FnMut(&K, &V, EntryState, VisibleRef) -> R,
        limit: usize,
    ) -> Vec<R>
    where
        K: Borrow<[u8]>,
    {
        if limit == 0 {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(limit);
        self.for_each_prefix(
            prefix,
            |key, value, state, visible| {
                out.push(reader(key, value, state, visible));
            },
            limit,
        );
        out
    }

    pub fn insert_dirty(&self, key: K, value: V) -> WriteId {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => {
                let record = self.dirty_mode.append(key, value);
                let id = record.id;
                self.publish_dirty_record(record);
                id
            }
            DirtyWriteMode::CoalescedMap => self.publish_coalesced_write(key, value),
        }
    }

    pub fn insert_dirty_batch(&self, entries: Vec<(K, V)>) -> Vec<WriteId> {
        if entries.is_empty() {
            return Vec::new();
        }

        // Keep the reservation guard alive across the full batch publication.
        let _reserved = self.visible.reserve(entries.len());

        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => self.insert_dirty_batch_strict(entries),
            DirtyWriteMode::CoalescedMap => self.insert_dirty_batch_coalesced(entries),
        }
    }

    pub fn insert_dirty_batch_without_ids(&self, entries: Vec<(K, V)>) -> usize {
        if entries.is_empty() {
            return 0;
        }

        // Keep the reservation guard alive across the full batch publication.
        let _reserved = self.visible.reserve(entries.len());

        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => self.insert_dirty_batch_strict_without_ids(entries),
            DirtyWriteMode::CoalescedMap => self.insert_dirty_batch_coalesced_without_ids(entries),
        }
    }

    pub fn upsert_dirty(&self, key: K, value: V) -> WriteId {
        self.insert_dirty(key, value)
    }

    pub fn insert_clean_if_absent(&self, key: K, value: V) -> Option<CacheId> {
        let id = self.next_cache.fetch_add(1, Ordering::Relaxed);
        let record = Arc::new(CleanRecord { id, key, value });
        let visible = VisibleValue::Clean(record.clone());
        let inserted = match self.visible.entry_sync(record.key.clone()) {
            MapEntry::Vacant(vacant) => {
                vacant.insert_entry(visible);
                self.clean_count.fetch_add(1, Ordering::Relaxed);
                true
            }
            MapEntry::Occupied(_) => false,
        };
        if inserted {
            self.enqueue_clean(record.key.clone(), id);
            Some(id)
        } else {
            None
        }
    }

    pub fn evict_clean<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.visible
            .remove_if_sync(key, |visible| matches!(visible, VisibleValue::Clean(_)))
            .map(|_| {
                self.clean_count.fetch_sub(1, Ordering::Relaxed);
                true
            })
            .unwrap_or(false)
    }

    pub fn cleanup_stale_visible<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        let Some(visible_ref) = self.visible_ref(key) else {
            return false;
        };
        self.cleanup_stale_visible_matching(key, visible_ref)
    }

    pub fn flush_batch(&self, limit: usize) -> FlushBatch<K, V> {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => self.dirty_mode.flush_batch(limit),
            DirtyWriteMode::CoalescedMap => self.flush_batch_coalesced(limit),
        }
    }

    pub fn mark_flushed(&self, batch: &FlushBatch<K, V>) -> usize {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => {
                let marked = self.dirty_mode.mark_flushed(batch);
                if marked == 0 {
                    return 0;
                }
                for record in batch.entries.iter() {
                    let id = record.id;
                    let _ = self.visible.remove_if_sync(&record.key, |visible| {
                        matches!(visible, VisibleValue::Dirty(current) if current.id == id)
                    });
                }
                marked
            }
            DirtyWriteMode::CoalescedMap => self.mark_flushed_coalesced(batch),
        }
    }

    fn cleanup_stale_visible_matching<Q>(&self, key: &Q, expected: VisibleRef) -> bool
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.visible
            .remove_if_sync(key, |visible| visible.visible_ref() == expected)
            .is_some()
    }

    fn publish_dirty_record(&self, record: Arc<DirtyRecord<K, V>>) {
        let visible_key = record.key.clone();
        let visible = VisibleValue::Dirty(record);
        match self.visible.entry_sync(visible_key) {
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
    }

    fn insert_dirty_batch_strict(&self, entries: Vec<(K, V)>) -> Vec<WriteId> {
        let records = self.dirty_mode.append_batch(entries);
        let mut ids = Vec::with_capacity(records.len());
        for record in records {
            ids.push(record.id);
            self.publish_dirty_record(record);
        }
        ids
    }

    fn insert_dirty_batch_strict_without_ids(&self, entries: Vec<(K, V)>) -> usize {
        let records = self.dirty_mode.append_batch(entries);
        let len = records.len();
        for record in records {
            self.publish_dirty_record(record);
        }
        len
    }

    fn insert_dirty_batch_coalesced(&self, entries: Vec<(K, V)>) -> Vec<WriteId> {
        if entries.is_empty() {
            return Vec::new();
        }

        let mut seen = AHashSet::with_capacity(entries.len());
        let mut has_duplicate = false;
        for (key, _) in &entries {
            if !seen.insert(key) {
                has_duplicate = true;
                break;
            }
        }

        if !has_duplicate {
            let mut ids = Vec::with_capacity(entries.len());
            for (key, value) in entries {
                ids.push(self.publish_coalesced_write(key, value));
            }
            return ids;
        }

        let mut latest = AHashMap::with_capacity(entries.len());
        for (key, value) in entries {
            let _ = latest.insert(key, value);
        }

        let mut ids = Vec::with_capacity(latest.len());
        for (key, value) in latest {
            ids.push(self.publish_coalesced_write(key, value));
        }
        ids
    }

    fn insert_dirty_batch_coalesced_without_ids(&self, entries: Vec<(K, V)>) -> usize {
        if entries.is_empty() {
            return 0;
        }

        let mut seen = AHashSet::with_capacity(entries.len());
        let mut has_duplicate = false;
        for (key, _) in &entries {
            if !seen.insert(key) {
                has_duplicate = true;
                break;
            }
        }

        if !has_duplicate {
            let len = entries.len();
            for (key, value) in entries {
                let _ = self.publish_coalesced_write(key, value);
            }
            return len;
        }

        let mut latest = AHashMap::with_capacity(entries.len());
        for (key, value) in entries {
            let _ = latest.insert(key, value);
        }

        let len = latest.len();
        for (key, value) in latest {
            let _ = self.publish_coalesced_write(key, value);
        }
        len
    }

    fn publish_coalesced_write(&self, key: K, value: V) -> WriteId {
        let id = self.next_write_id();
        let record = Arc::new(DirtyRecord { id, key, value });
        self.publish_dirty_record_coalesced(record);
        id
    }

    fn publish_dirty_record_coalesced(&self, record: Arc<DirtyRecord<K, V>>) {
        let visible_key = record.key.clone();
        let visible = VisibleValue::Dirty(record);
        match self.visible.entry_sync(visible_key) {
            MapEntry::Occupied(mut occupied) => {
                if matches!(occupied.get(), VisibleValue::Clean(_)) {
                    self.clean_count.fetch_sub(1, Ordering::Relaxed);
                    self.coalesced_dirty_count.fetch_add(1, Ordering::Relaxed);
                }
                let _ = occupied.insert(visible);
            }
            MapEntry::Vacant(vacant) => {
                vacant.insert_entry(visible);
                self.coalesced_dirty_count.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn next_write_id(&self) -> WriteId {
        self.next_dirty_write.fetch_add(1, Ordering::Relaxed)
    }

    fn flush_batch_coalesced(&self, limit: usize) -> FlushBatch<K, V> {
        if limit == 0 {
            return FlushBatch::new(Vec::new());
        }

        {
            let inflight = lock(&self.coalesced_inflight);
            if let Some(batch) = inflight.as_ref() {
                return batch.clone();
            }
        }

        let mut entries = Vec::with_capacity(limit);
        self.visible.iter_sync(|_, visible| {
            if entries.len() >= limit {
                return false;
            }
            if let VisibleValue::Dirty(record) = visible {
                entries.push(record.clone());
            }
            entries.len() < limit
        });

        let batch = FlushBatch::new(entries);
        if batch.is_empty() {
            return batch;
        }

        let mut inflight = lock(&self.coalesced_inflight);
        if let Some(current) = inflight.as_ref() {
            return current.clone();
        }
        *inflight = Some(batch.clone());
        batch
    }

    fn mark_flushed_coalesced(&self, batch: &FlushBatch<K, V>) -> usize {
        let mut inflight = lock(&self.coalesced_inflight);
        let Some(current) = inflight.as_ref() else {
            return 0;
        };
        if !Self::same_batch_ids(current, batch) {
            return 0;
        }
        let drained = inflight.take().expect("coalesced inflight vanished");
        drop(inflight);

        let mut removed = 0_usize;
        for record in drained.entries.iter() {
            let id = record.id;
            if self
                .visible
                .remove_if_sync(
                    &record.key,
                    |visible| matches!(visible, VisibleValue::Dirty(current) if current.id == id),
                )
                .is_some()
            {
                removed += 1;
            }
        }
        if removed != 0 {
            self.coalesced_dirty_count
                .fetch_sub(removed, Ordering::Relaxed);
        }
        drained.len()
    }

    fn same_batch_ids(left: &FlushBatch<K, V>, right: &FlushBatch<K, V>) -> bool {
        left.len() == right.len()
            && left
                .entries
                .iter()
                .zip(right.entries.iter())
                .all(|(l, r)| l.id == r.id)
    }

    fn enqueue_clean(&self, key: K, id: CacheId) {
        let mut pending = (key, id);
        loop {
            match self.clean_fifo.push(pending) {
                Ok(()) => break,
                Err(returned) => {
                    pending = returned;
                    if let Some((old_key, old_id)) = self.clean_fifo.pop() {
                        let removed = self.visible.remove_if_sync(&old_key, |visible| {
                            matches!(visible, VisibleValue::Clean(record) if record.id == old_id)
                        });
                        if removed.is_some() {
                            self.clean_count.fetch_sub(1, Ordering::Relaxed);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(any(test, feature = "loom"))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DebugRecord<K, V> {
    pub id: u64,
    pub key: K,
    pub value: V,
}

#[cfg(any(test, feature = "loom"))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DebugVisible<K, V> {
    Dirty(DebugRecord<K, V>),
    Clean(DebugRecord<K, V>),
}

#[cfg(any(test, feature = "loom"))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DebugSnapshot<K, V> {
    pub visible: Vec<(K, DebugVisible<K, V>)>,
    pub dirty_pending: Vec<DebugRecord<K, V>>,
    pub dirty_inflight: Vec<DebugRecord<K, V>>,
    pub next_write: u64,
    pub next_cache: u64,
    pub clean_count: usize,
}

#[cfg(any(test, feature = "loom"))]
impl<K, V, H> CacheLogMap<K, V, H>
where
    K: Clone + Eq + Hash + Ord,
    V: Clone,
    H: BuildHasher + Clone,
{
    #[doc(hidden)]
    pub fn debug_snapshot(&self) -> DebugSnapshot<K, V> {
        let mut visible = Vec::new();
        self.visible.iter_sync(|key, value| {
            let projected = match value {
                VisibleValue::Dirty(record) => DebugVisible::Dirty(DebugRecord {
                    id: record.id,
                    key: record.key.clone(),
                    value: record.value.clone(),
                }),
                VisibleValue::Clean(record) => DebugVisible::Clean(DebugRecord {
                    id: record.id,
                    key: record.key.clone(),
                    value: record.value.clone(),
                }),
            };
            visible.push((key.clone(), projected));
            true
        });
        visible.sort_by(|left, right| left.0.cmp(&right.0));

        let (dirty_pending, dirty_inflight) = match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => {
                let pending = self
                    .dirty_mode
                    .pending_records()
                    .into_iter()
                    .map(|record| DebugRecord {
                        id: record.id,
                        key: record.key.clone(),
                        value: record.value.clone(),
                    })
                    .collect();
                let inflight = self
                    .dirty_mode
                    .inflight_records()
                    .into_iter()
                    .map(|record| DebugRecord {
                        id: record.id,
                        key: record.key.clone(),
                        value: record.value.clone(),
                    })
                    .collect();
                (pending, inflight)
            }
            DirtyWriteMode::CoalescedMap => {
                let inflight_records = lock(&self.coalesced_inflight)
                    .as_ref()
                    .map(|batch| {
                        batch
                            .entries
                            .iter()
                            .map(|record| DebugRecord {
                                id: record.id,
                                key: record.key.clone(),
                                value: record.value.clone(),
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let inflight_ids = inflight_records
                    .iter()
                    .map(|record| record.id)
                    .collect::<std::collections::BTreeSet<_>>();
                let pending_records = visible
                    .iter()
                    .filter_map(|(_, projected)| match projected {
                        DebugVisible::Dirty(record) if !inflight_ids.contains(&record.id) => {
                            Some(record.clone())
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                (pending_records, inflight_records)
            }
        };

        DebugSnapshot {
            visible,
            dirty_pending,
            dirty_inflight,
            next_write: match self.dirty_write_mode {
                DirtyWriteMode::StrictLog => self.dirty_mode.next_write(),
                DirtyWriteMode::CoalescedMap => self.next_dirty_write.load(Ordering::Relaxed),
            },
            next_cache: self.next_cache.load(Ordering::Relaxed),
            clean_count: self.clean_count.load(Ordering::Relaxed),
        }
    }
}

#[cfg(all(test, not(feature = "loom")))]
mod conformance_tests {
    use std::collections::BTreeMap;

    use cachelog_core::{ModelConfig, ModelState, VisibleRef as ModelVisibleRef};
    use cachelog_model::{
        ComparableCleanRecord, ComparableDirtyRecord, ComparableState, ComparableVisibleRef,
    };

    use super::*;

    fn to_core_id(id: u64) -> usize {
        usize::try_from(id).expect("id does not fit in usize")
    }

    struct ConformanceHarness {
        live: CacheLogMap<usize, usize>,
        model: ModelState,
        model_cfg: ModelConfig,
        inflight: Vec<usize>,
    }

    impl ConformanceHarness {
        fn new() -> Self {
            let model_cfg = ModelConfig::new(8, 64, 64, 64);
            Self {
                live: CacheLogMap::new(CacheLogConfig::new(64, 64, 64)),
                model: ModelState::new(model_cfg),
                model_cfg,
                inflight: Vec::new(),
            }
        }

        fn insert_dirty(&mut self, key: usize, value: usize) -> usize {
            let previous = self.model.visible[key];
            let live_id = self.live.insert_dirty(key, value);
            self.model
                .writer_write(self.model_cfg, key, value)
                .expect("model writer_write failed");
            if let ModelVisibleRef::Clean(id) = previous {
                self.model
                    .cache_drop_record(self.model_cfg, id)
                    .expect("model cache_drop_record failed");
                self.model
                    .cache_cleanup_visible(self.model_cfg, id)
                    .expect("model cache_cleanup_visible failed");
            }
            assert_eq!(to_core_id(live_id), self.model.next_write - 1);
            self.assert_matches_model();
            to_core_id(live_id)
        }

        fn insert_clean_from_durable(&mut self, key: usize) -> Option<usize> {
            let durable = &self.model.durable[key];
            assert!(durable.present, "no durable value for key {key}");
            let expected_id = self.model.next_cache;
            let result = self.live.insert_clean_if_absent(key, durable.value);
            self.model
                .cache_insert(self.model_cfg, key)
                .expect("model cache_insert failed");
            self.assert_matches_model();
            result.map(|id| {
                let id = to_core_id(id);
                assert_eq!(id, expected_id);
                id
            })
        }

        fn evict_clean(&mut self, key: usize) -> bool {
            let current = self.model.visible[key];
            let removed = self.live.evict_clean(&key);
            if let ModelVisibleRef::Clean(id) = current {
                self.model
                    .cache_drop_record(self.model_cfg, id)
                    .expect("model cache_drop_record failed");
                self.model
                    .cache_cleanup_visible(self.model_cfg, id)
                    .expect("model cache_cleanup_visible failed");
                assert!(removed);
            } else {
                assert!(!removed);
            }
            self.assert_matches_model();
            removed
        }

        fn flush_batch(&mut self, limit: usize) -> Vec<usize> {
            let batch = self.live.flush_batch(limit);
            let ids = batch
                .iter()
                .map(|record| to_core_id(record.id))
                .collect::<Vec<_>>();
            if self.inflight.is_empty() {
                let model_queue = self.model.dirty_q.clone();
                assert_eq!(
                    ids,
                    model_queue.iter().copied().take(limit).collect::<Vec<_>>()
                );
            } else {
                assert_eq!(ids, self.inflight);
            }
            self.inflight = ids.clone();
            self.assert_matches_model();
            ids
        }

        fn mark_flushed(&mut self, limit: usize) -> Vec<usize> {
            let batch = self.live.flush_batch(limit);
            let ids = batch
                .iter()
                .map(|record| to_core_id(record.id))
                .collect::<Vec<_>>();
            assert_eq!(ids, self.inflight);
            let marked = self.live.mark_flushed(&batch);
            assert_eq!(marked, ids.len());
            drop(batch);
            for &id in &ids {
                let expected = *self.model.dirty_q.first().expect("model dirty_q empty");
                assert_eq!(expected, id);
                self.model
                    .flusher_flush_next(self.model_cfg)
                    .expect("model flusher_flush_next failed");
                self.model
                    .flusher_drop(self.model_cfg, id)
                    .expect("model flusher_drop failed");
            }
            self.inflight.clear();
            self.assert_matches_model();
            ids
        }

        fn assert_public_behavior_matches_model(&self) {
            for key in 0..self.model_cfg.key_count {
                let expected_visible = match self.model.visible[key] {
                    ModelVisibleRef::None => None,
                    ModelVisibleRef::Dirty(id) => Some(ComparableVisibleRef::Dirty(id)),
                    ModelVisibleRef::Clean(id) => Some(ComparableVisibleRef::Clean(id)),
                };
                let actual_visible = self.live.visible_ref(&key).map(|visible| match visible {
                    VisibleRef::Dirty(id) => ComparableVisibleRef::Dirty(to_core_id(id)),
                    VisibleRef::Clean(id) => ComparableVisibleRef::Clean(to_core_id(id)),
                });
                assert_eq!(
                    actual_visible, expected_visible,
                    "visible_ref mismatch for key {key}"
                );

                let expected_triplet = match self.model.visible[key] {
                    ModelVisibleRef::None => None,
                    ModelVisibleRef::Dirty(id) => {
                        let record = &self.model.write_store[id];
                        Some((
                            record.value,
                            EntryState::Dirty,
                            ComparableVisibleRef::Dirty(id),
                        ))
                    }
                    ModelVisibleRef::Clean(id) => {
                        let record = &self.model.cache_store[id];
                        Some((
                            record.value,
                            EntryState::Clean,
                            ComparableVisibleRef::Clean(id),
                        ))
                    }
                };
                let actual_triplet = self.live.read(&key, |_, value, state, visible| {
                    let visible = match visible {
                        VisibleRef::Dirty(id) => ComparableVisibleRef::Dirty(to_core_id(id)),
                        VisibleRef::Clean(id) => ComparableVisibleRef::Clean(to_core_id(id)),
                    };
                    (*value, state, visible)
                });
                assert_eq!(
                    actual_triplet, expected_triplet,
                    "read mismatch for key {key}"
                );
                assert_eq!(
                    self.live.contains(&key),
                    expected_triplet.is_some(),
                    "contains mismatch for key {key}"
                );
            }
        }

        fn assert_matches_model(&self) {
            let snapshot = self.live.debug_snapshot();
            let comparable = ComparableState::from(&self.model);

            let visible = snapshot
                .visible
                .iter()
                .map(|(key, visible)| {
                    let visible = match visible {
                        DebugVisible::Dirty(record) => {
                            ComparableVisibleRef::Dirty(to_core_id(record.id))
                        }
                        DebugVisible::Clean(record) => {
                            ComparableVisibleRef::Clean(to_core_id(record.id))
                        }
                    };
                    (*key, visible)
                })
                .collect::<BTreeMap<_, _>>();
            assert_eq!(visible, comparable.visible);

            let mut write_store = BTreeMap::new();
            for (_, visible) in &snapshot.visible {
                if let DebugVisible::Dirty(record) = visible {
                    write_store.insert(
                        to_core_id(record.id),
                        ComparableDirtyRecord {
                            id: to_core_id(record.id),
                            key: record.key,
                            value: record.value,
                        },
                    );
                }
            }
            for record in snapshot
                .dirty_pending
                .iter()
                .chain(snapshot.dirty_inflight.iter())
            {
                write_store.insert(
                    to_core_id(record.id),
                    ComparableDirtyRecord {
                        id: to_core_id(record.id),
                        key: record.key,
                        value: record.value,
                    },
                );
            }
            assert_eq!(write_store, comparable.write_store);

            let queue_total = snapshot
                .dirty_inflight
                .iter()
                .chain(snapshot.dirty_pending.iter())
                .map(|record| to_core_id(record.id))
                .collect::<Vec<_>>();
            assert_eq!(queue_total, comparable.dirty_q);
            assert_eq!(
                snapshot
                    .dirty_inflight
                    .iter()
                    .map(|record| to_core_id(record.id))
                    .collect::<Vec<_>>(),
                self.inflight
            );

            let cache_store = snapshot
                .visible
                .iter()
                .filter_map(|(_, visible)| match visible {
                    DebugVisible::Clean(record) => Some((
                        to_core_id(record.id),
                        ComparableCleanRecord {
                            id: to_core_id(record.id),
                            key: record.key,
                            value: record.value,
                        },
                    )),
                    DebugVisible::Dirty(_) => None,
                })
                .collect::<BTreeMap<_, _>>();
            assert_eq!(cache_store, comparable.cache_store);
            assert_eq!(snapshot.next_write, self.live.dirty_mode.next_write());
            assert_eq!(to_core_id(snapshot.next_write), comparable.next_write);
            assert_eq!(to_core_id(snapshot.next_cache), comparable.next_cache);
            assert_eq!(snapshot.clean_count, cache_store.len());
            self.assert_public_behavior_matches_model();
        }
    }

    #[test]
    fn conformance_dirty_write_then_read() {
        let mut harness = ConformanceHarness::new();
        harness.insert_dirty(1, 11);
    }

    #[test]
    fn conformance_clean_insert_after_flush() {
        let mut harness = ConformanceHarness::new();
        harness.insert_dirty(2, 22);
        harness.flush_batch(1);
        harness.mark_flushed(1);
        assert_eq!(harness.insert_clean_from_durable(2), Some(0));
    }

    #[test]
    fn conformance_dirty_over_clean_replacement() {
        let mut harness = ConformanceHarness::new();
        harness.insert_dirty(3, 30);
        harness.flush_batch(1);
        harness.mark_flushed(1);
        assert_eq!(harness.insert_clean_from_durable(3), Some(0));
        harness.insert_dirty(3, 31);
    }

    #[test]
    fn conformance_flush_old_dirty_keeps_newer_visible() {
        let mut harness = ConformanceHarness::new();
        harness.insert_dirty(1, 10);
        harness.insert_dirty(1, 20);
        assert_eq!(harness.flush_batch(1), vec![0]);
        assert_eq!(harness.mark_flushed(1), vec![0]);
    }

    #[test]
    fn conformance_inflight_batch_reuse_matches_model_queue() {
        let mut harness = ConformanceHarness::new();
        harness.insert_dirty(0, 1);
        harness.insert_dirty(1, 2);
        let first = harness.flush_batch(1);
        let second = harness.flush_batch(2);
        assert_eq!(first, second);
        harness.assert_matches_model();
    }

    #[test]
    fn conformance_clean_eviction_matches_model() {
        let mut harness = ConformanceHarness::new();
        harness.insert_dirty(4, 40);
        harness.flush_batch(1);
        harness.mark_flushed(1);
        harness.insert_clean_from_durable(4);
        assert!(harness.evict_clean(4));
    }

    #[test]
    fn conformance_interleaved_partial_flush() {
        let mut harness = ConformanceHarness::new();
        harness.insert_dirty(0, 1);
        harness.insert_dirty(1, 2);
        harness.insert_dirty(0, 3);
        assert_eq!(harness.flush_batch(2), vec![0, 1]);
        assert_eq!(harness.mark_flushed(2), vec![0, 1]);
        harness.insert_dirty(2, 4);
        harness.assert_matches_model();
    }
}
