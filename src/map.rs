use ahash::{AHashMap, AHashSet, RandomState as AHashRandomState};
#[cfg(any(test, feature = "dev-tools"))]
use bytes::Bytes;
use std::borrow::Borrow;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hash};
#[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
use std::time::Instant as StdInstant;

use scc::HashMap as ConcurrentHashMap;
use scc::hash_map::Entry as MapEntry;

#[cfg(not(feature = "loom"))]
use crate::background_flush::{
    BackgroundFlushConfig, BackgroundFlushHandle, BackgroundFlushService,
};
#[cfg(any(test, feature = "dev-tools"))]
use crate::bytes_pooling::bytes_from_borrowed;
use crate::dirty_mode::{DirtyMode, OrderedFifoDirty};
use crate::entry::{
    CacheId, CleanRecord, DirtyRecord, EntryState, FlushBatch, PersistBatch, VisibleRef, WriteId,
};
#[cfg(feature = "dev-tools")]
use crate::perf::CoalescedSingleWritePerfCounters;
#[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
use crate::perf::*;
use crate::sync::{
    Arc, AtomicU64, AtomicUsize, BoundedQueue, Mutex, Ordering, SharedArc, SharedOptionArc, lock,
    new_mutex,
};

pub(crate) mod engine;
#[cfg(not(feature = "loom"))]
pub(crate) use engine::{CoalescedArcEngine, StrictArcEngine};
pub(crate) use engine::{CoalescedEngine, StrictEngine};

const COALESCED_UNASSIGNED_WRITE_ID: WriteId = u64::MAX - 1;

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

#[derive(Clone, Debug)]
struct CoalescedInflight<K, V> {
    batch: FlushBatch<K, V>,
    index: AHashMap<K, usize>,
}

#[derive(Clone, Debug)]
struct CoalescedRecord<V> {
    id: WriteId,
    value: V,
}

type CoalescedActiveMap<K, V, H> = ConcurrentHashMap<K, Box<CoalescedRecord<V>>, H>;
type SharedCoalescedMap<K, V, H> = Arc<CoalescedActiveMap<K, V, H>>;

struct CoalescedOverlay<K, V, H>
where
    K: Clone + Eq + Hash,
    H: BuildHasher + Clone,
{
    active: SharedArc<CoalescedActiveMap<K, V, H>>,
    active_publish: Mutex<()>,
    draining: Mutex<Option<SharedCoalescedMap<K, V, H>>>,
    draining_shared: SharedOptionArc<CoalescedActiveMap<K, V, H>>,
    inflight: Mutex<Option<Arc<CoalescedInflight<K, V>>>>,
    inflight_shared: SharedOptionArc<CoalescedInflight<K, V>>,
    compact_scratch: BoundedQueue<CoalescedCompactScratch<K, V>>,
    build_hasher: H,
    map_capacity: usize,
}

impl<K, V, H> CoalescedOverlay<K, V, H>
where
    K: Clone + Eq + Hash,
    H: BuildHasher + Clone,
{
    fn dirty_len(&self) -> usize {
        self.active.load_full().len()
            + lock(&self.inflight)
                .as_ref()
                .map_or(0, |current| current.batch.len())
            + lock(&self.draining)
                .as_ref()
                .map_or(0, |snapshot| snapshot.len())
    }

    fn read_visible_ref<Q>(&self, key: &Q) -> Option<VisibleRef>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.read_active(key, |_, record| VisibleRef::Dirty(record.id))
            .or_else(|| self.read_inflight(key, |record| VisibleRef::Dirty(record.id)))
            .or_else(|| self.read_draining(key, |_, record| VisibleRef::Dirty(record.id)))
    }

    fn read_active<Q, R>(
        &self,
        key: &Q,
        reader: impl FnOnce(&K, &CoalescedRecord<V>) -> R,
    ) -> Option<R>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.active.with(|active| {
            active.read_sync(key, |stored_key, record| {
                reader(stored_key, record.as_ref())
            })
        })
    }

    fn read_draining<Q, R>(
        &self,
        key: &Q,
        reader: impl FnOnce(&K, &CoalescedRecord<V>) -> R,
    ) -> Option<R>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
        let draining_lock_started = coalesced_perf_enabled().then(StdInstant::now);
        let result = self.draining_shared.with(|draining| {
            let draining = draining?;
            #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
            let draining_read_started = coalesced_perf_enabled().then(StdInstant::now);
            let result = draining.read_sync(key, |stored_key, record| {
                reader(stored_key, record.as_ref())
            });
            #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
            if let Some(started) = draining_read_started {
                PERF_COALESCED_SINGLE_SNAPSHOT_DRAINING_READ_NS_TOTAL
                    .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
            }
            Some(result)
        });
        #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
        if let Some(started) = draining_lock_started {
            PERF_COALESCED_SINGLE_SNAPSHOT_DRAINING_LOCK_NS_TOTAL
                .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
        result?
    }

    fn for_each_active_key(&self, mut reader: impl FnMut(&K))
    where
        K: Clone,
    {
        self.active.with(|active| {
            active.iter_sync(|key, _| {
                reader(key);
                true
            })
        });
    }

    fn for_each_draining_key(&self, mut reader: impl FnMut(&K))
    where
        K: Clone,
    {
        self.draining_shared.with(|draining| {
            let Some(draining) = draining else {
                return;
            };
            draining.iter_sync(|key, _| {
                reader(key);
                true
            });
        });
    }

    fn visible_key_count(&self) -> usize
    where
        K: Clone,
    {
        let mut seen = AHashSet::new();
        self.for_each_active_key(|key| {
            seen.insert(key.clone());
        });
        self.with_inflight(|inflight| {
            if let Some(inflight) = inflight {
                for record in inflight.batch.iter() {
                    seen.insert(record.key.clone());
                }
            }
        });
        self.for_each_draining_key(|key| {
            seen.insert(key.clone());
        });
        seen.len()
    }

    fn take_compact_scratch(&self, capacity: usize) -> CoalescedCompactScratch<K, V> {
        let mut scratch = self
            .compact_scratch
            .pop()
            .unwrap_or_else(|| CoalescedCompactScratch::with_capacity(capacity));
        scratch.ensure_capacity(capacity);
        scratch
    }

    fn return_compact_scratch(&self, mut scratch: CoalescedCompactScratch<K, V>) {
        scratch.clear();
        let _ = self.compact_scratch.push(scratch);
    }

    fn publish_owned(
        map: &ConcurrentHashMap<K, Box<CoalescedRecord<V>>, H>,
        key: K,
        id: WriteId,
        value: V,
    ) -> bool {
        #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
        let entry_lookup_started = coalesced_perf_enabled().then(StdInstant::now);
        let entry = map.entry_sync(key);
        #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
        if let Some(started) = entry_lookup_started {
            PERF_COALESCED_SINGLE_ACTIVE_ENTRY_LOOKUP_NS_TOTAL
                .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
        match entry {
            MapEntry::Occupied(mut occupied) => {
                #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
                let occupied_started = coalesced_perf_enabled().then(StdInstant::now);
                let current = occupied.get_mut();
                current.id = id;
                current.value = value;
                #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
                if let Some(started) = occupied_started {
                    PERF_COALESCED_SINGLE_ACTIVE_ENTRY_OCCUPIED_NS_TOTAL
                        .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
                }
                false
            }
            MapEntry::Vacant(vacant) => {
                #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
                let vacant_started = coalesced_perf_enabled().then(StdInstant::now);
                #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
                let vacant_box_started = coalesced_perf_enabled().then(StdInstant::now);
                let record = Box::new(CoalescedRecord { id, value });
                #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
                if let Some(started) = vacant_box_started {
                    PERF_COALESCED_SINGLE_ACTIVE_ENTRY_VACANT_BOX_NS_TOTAL
                        .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
                }
                #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
                let vacant_insert_started = coalesced_perf_enabled().then(StdInstant::now);
                vacant.insert_entry(record);
                #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
                if let Some(started) = vacant_insert_started {
                    PERF_COALESCED_SINGLE_ACTIVE_ENTRY_VACANT_INSERT_NS_TOTAL
                        .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
                }
                #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
                if let Some(started) = vacant_started {
                    PERF_COALESCED_SINGLE_ACTIVE_ENTRY_VACANT_NS_TOTAL
                        .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
                }
                true
            }
        }
    }

    fn publish_active_locked(&self, key: K, id: WriteId, value: V) -> bool {
        #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
        let publish_started = coalesced_perf_enabled().then(StdInstant::now);
        #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
        let active_load_started = coalesced_perf_enabled().then(StdInstant::now);
        let inserted = self.active.with(|active| {
            #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
            if let Some(started) = active_load_started {
                PERF_COALESCED_SINGLE_ACTIVE_MAP_LOAD_NS_TOTAL
                    .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
            }
            #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
            let active_entry_started = coalesced_perf_enabled().then(StdInstant::now);
            let inserted = match active.insert_sync(key, Box::new(CoalescedRecord { id, value })) {
                Ok(()) => true,
                Err((key, record)) => Self::publish_owned(active, key, record.id, record.value),
            };
            #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
            if let Some(started) = active_entry_started {
                PERF_COALESCED_SINGLE_ACTIVE_ENTRY_PUBLISH_NS_TOTAL
                    .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
            }
            inserted
        });
        #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
        if let Some(started) = publish_started {
            PERF_COALESCED_SINGLE_ACTIVE_PUBLISH_NS_TOTAL
                .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
        inserted
    }

    fn update_or_insert<Q>(&self, lookup_key: &Q, owned_key: K, id: WriteId, value: V) -> bool
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        let mut value = Some(value);
        self.active.with(|active| {
            self.update_or_insert_in_active(active, lookup_key, owned_key, id, &mut value)
        })
    }

    fn update_or_insert_in_active<Q>(
        &self,
        active: &ConcurrentHashMap<K, Box<CoalescedRecord<V>>, H>,
        lookup_key: &Q,
        owned_key: K,
        id: WriteId,
        value: &mut Option<V>,
    ) -> bool
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        if active
            .update_sync(lookup_key, |_, current| {
                current.id = id;
                current.value = value.take().expect("coalesced update value should exist");
            })
            .is_some()
        {
            return false;
        }

        match active.entry_sync(owned_key) {
            MapEntry::Occupied(mut occupied) => {
                let current = occupied.get_mut();
                current.id = id;
                current.value = value.take().expect("coalesced raced value should exist");
                false
            }
            MapEntry::Vacant(vacant) => {
                vacant.insert_entry(Box::new(CoalescedRecord {
                    id,
                    value: value.take().expect("coalesced miss value should exist"),
                }));
                true
            }
        }
    }

    fn new_active_map(&self) -> SharedCoalescedMap<K, V, H> {
        Arc::new(ConcurrentHashMap::with_capacity_and_hasher(
            self.map_capacity,
            self.build_hasher.clone(),
        ))
    }

    fn ensure_draining_snapshot_locked(&self) {
        {
            let draining = lock(&self.draining);
            if draining.as_ref().is_some_and(|map| !map.is_empty()) {
                return;
            }
        }

        let active = self.active.load_full();
        if active.is_empty() {
            return;
        }

        let fresh = self.new_active_map();
        let drained = self.active.swap(fresh);
        let mut draining = lock(&self.draining);
        if draining.as_ref().is_some_and(|map| !map.is_empty()) {
            return;
        }
        self.draining_shared.store(Some(drained.clone()));
        *draining = Some(drained);
    }

    fn begin_snapshot_drain_batch(&self, limit: usize) -> Vec<DirtyRecord<K, V>> {
        let mut draining = lock(&self.draining);
        let Some(snapshot) = draining.as_ref() else {
            return Vec::new();
        };
        if snapshot.is_empty() {
            self.draining_shared.store(None);
            *draining = None;
            return Vec::new();
        }

        let mut entries = Vec::with_capacity(limit);
        let mut current = snapshot.begin_sync();
        while entries.len() < limit {
            let Some(entry) = current.take() else {
                break;
            };
            let ((key, record), next) = entry.remove_and_sync();
            entries.push(DirtyRecord {
                id: record.id,
                key,
                value: record.value,
            });
            current = next;
        }
        if snapshot.is_empty() {
            self.draining_shared.store(None);
            *draining = None;
        }
        entries
    }

    fn mark_flushed_if_current(&self, batch: &FlushBatch<K, V>) -> usize {
        let mut inflight = lock(&self.inflight);
        let Some(current) = inflight.as_ref() else {
            return 0;
        };
        if !Self::same_batch_ids(&current.batch, batch) {
            return 0;
        }
        let drained = inflight.take().expect("coalesced inflight vanished");
        self.inflight_shared.store(None);
        drop(inflight);

        let mut draining = lock(&self.draining);
        if draining
            .as_ref()
            .is_some_and(|snapshot| snapshot.is_empty())
        {
            self.draining_shared.store(None);
            *draining = None;
        }

        drained.batch.len()
    }

    fn build_inflight_index(batch: &FlushBatch<K, V>) -> AHashMap<K, usize> {
        let mut index = AHashMap::with_capacity(batch.len());
        for (slot, record) in batch.iter().enumerate() {
            index.insert(record.key.clone(), slot);
        }
        index
    }

    fn read_inflight<Q, R>(
        &self,
        key: &Q,
        reader: impl FnOnce(&DirtyRecord<K, V>) -> R,
    ) -> Option<R>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
        let inflight_lock_started = coalesced_perf_enabled().then(StdInstant::now);
        let result = self.inflight_shared.with(|inflight| {
            let inflight = inflight?;
            #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
            let inflight_index_lookup_started = coalesced_perf_enabled().then(StdInstant::now);
            let slot = inflight.index.get(key).copied();
            #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
            if let Some(started) = inflight_index_lookup_started {
                PERF_COALESCED_SINGLE_SNAPSHOT_INFLIGHT_INDEX_LOOKUP_NS_TOTAL
                    .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
            }
            let slot = slot?;
            #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
            let inflight_record_get_started = coalesced_perf_enabled().then(StdInstant::now);
            let record = inflight.batch.get(slot);
            #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
            if let Some(started) = inflight_record_get_started {
                PERF_COALESCED_SINGLE_SNAPSHOT_INFLIGHT_RECORD_GET_NS_TOTAL
                    .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
            }
            let record = record.expect("coalesced inflight index pointed past batch len");
            Some(reader(record))
        });
        #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
        if let Some(started) = inflight_lock_started {
            PERF_COALESCED_SINGLE_SNAPSHOT_INFLIGHT_LOCK_NS_TOTAL
                .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
        result
    }

    fn with_inflight<R>(&self, reader: impl FnOnce(Option<&CoalescedInflight<K, V>>) -> R) -> R {
        self.inflight_shared
            .with(|inflight| reader(inflight.map(Arc::as_ref)))
    }

    fn backlog_counts(&self) -> DirtyBacklogCounts {
        let pending_visible = self.active.load_full().len();
        let inflight = lock(&self.inflight)
            .as_ref()
            .map_or(0, |current| current.batch.len());
        let draining = lock(&self.draining)
            .as_ref()
            .map_or(0, |snapshot| snapshot.len());
        DirtyBacklogCounts {
            dirty_total: self.dirty_len(),
            pending_visible,
            inflight,
            draining,
        }
    }

    fn same_batch_ids(left: &FlushBatch<K, V>, right: &FlushBatch<K, V>) -> bool {
        left.ptr_eq(right)
            || (left.len() == right.len()
                && left.iter().zip(right.iter()).all(|(l, r)| l.id == r.id))
    }
}

#[derive(Debug)]
struct CoalescedCompactScratch<K, V> {
    latest_slot: AHashMap<u64, DedupeSlots>,
    compacted: Vec<(K, V)>,
    fingerprint_hasher: AHashRandomState,
}

impl<K, V> CoalescedCompactScratch<K, V> {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            latest_slot: AHashMap::with_capacity(capacity),
            compacted: Vec::with_capacity(capacity),
            fingerprint_hasher: AHashRandomState::new(),
        }
    }

    fn ensure_capacity(&mut self, capacity: usize) {
        let latest_slot_capacity = self.latest_slot.capacity();
        if latest_slot_capacity < capacity {
            self.latest_slot
                .reserve(capacity.saturating_sub(latest_slot_capacity));
        }
        let compacted_capacity = self.compacted.capacity();
        if compacted_capacity < capacity {
            self.compacted
                .reserve(capacity.saturating_sub(compacted_capacity));
        }
    }

    fn clear(&mut self) {
        self.latest_slot.clear();
        self.compacted.clear();
    }
}

#[derive(Clone, Debug)]
enum DedupeSlots {
    One(usize),
    Many(Vec<usize>),
}

pub(crate) mod borrowed_keys {
    #[cfg(not(feature = "loom"))]
    use std::{cell::RefCell, thread_local};

    #[cfg(not(feature = "loom"))]
    thread_local! {
        static BORROWED_COALESCED_KEYS: RefCell<Vec<Vec<u8>>> = const { RefCell::new(Vec::new()) };
    }

    pub(crate) fn with_borrowed_coalesced_keys<R>(f: impl FnOnce(&mut Vec<Vec<u8>>) -> R) -> R {
        #[cfg(not(feature = "loom"))]
        {
            BORROWED_COALESCED_KEYS.with(|scratch| f(&mut scratch.borrow_mut()))
        }

        #[cfg(feature = "loom")]
        {
            let mut scratch = Vec::new();
            f(&mut scratch)
        }
    }
}

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

pub struct CacheLogMap<K, V, H = RandomState>
where
    K: Clone + Eq + Hash,
    H: BuildHasher + Clone,
{
    visible: ConcurrentHashMap<K, VisibleValue<K, V>, H>,
    coalesced: CoalescedOverlay<K, V, H>,
    dirty_mode: OrderedFifoDirty<K, V>,
    dirty_write_mode: DirtyWriteMode,
    clean_fifo: BoundedQueue<(K, CacheId)>,
    clean_count: AtomicUsize,
    next_cache: AtomicU64,
    next_dirty_write: AtomicU64,
    coalesced_has_unassigned_ids: AtomicUsize,
    strict_flushed_upto: AtomicU64,
}

pub struct LowLevelMap<'a, K, V, H = RandomState>
where
    K: Clone + Eq + Hash,
    H: BuildHasher + Clone,
{
    map: &'a CacheLogMap<K, V, H>,
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
    const COALESCED_COMPACT_SCRATCH_POOL_SIZE: usize = 4;

    pub fn with_hasher(config: CacheLogConfig, build_hasher: H) -> Self {
        let initial_compact_capacity = config.dirty_log_capacity.clamp(64, 1024);
        let compact_scratch = BoundedQueue::new(Self::COALESCED_COMPACT_SCRATCH_POOL_SIZE.max(1));
        for _ in 0..Self::COALESCED_COMPACT_SCRATCH_POOL_SIZE {
            let _ = compact_scratch.push(CoalescedCompactScratch::with_capacity(
                initial_compact_capacity,
            ));
        }

        Self {
            visible: ConcurrentHashMap::with_capacity_and_hasher(
                config.visible_capacity,
                build_hasher.clone(),
            ),
            coalesced: CoalescedOverlay {
                active: SharedArc::new(Arc::new(ConcurrentHashMap::with_capacity_and_hasher(
                    config.visible_capacity,
                    build_hasher.clone(),
                ))),
                active_publish: new_mutex(()),
                draining: new_mutex(None),
                draining_shared: SharedOptionArc::empty(),
                inflight: new_mutex(None),
                inflight_shared: SharedOptionArc::empty(),
                compact_scratch,
                build_hasher: build_hasher.clone(),
                map_capacity: config.visible_capacity,
            },
            dirty_mode: OrderedFifoDirty::new(config.dirty_log_capacity),
            dirty_write_mode: config.dirty_write_mode,
            clean_fifo: BoundedQueue::new(config.clean_capacity),
            clean_count: AtomicUsize::new(0),
            next_cache: AtomicU64::new(0),
            next_dirty_write: AtomicU64::new(0),
            coalesced_has_unassigned_ids: AtomicUsize::new(0),
            strict_flushed_upto: AtomicU64::new(u64::MAX),
        }
    }

    pub fn visible_len(&self) -> usize {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => self.visible.len(),
            DirtyWriteMode::CoalescedMap => {
                self.clean_count.load(Ordering::Relaxed) + self.coalesced.visible_key_count()
            }
        }
    }

    pub fn dirty_log_len(&self) -> usize {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => self.dirty_mode.len(),
            DirtyWriteMode::CoalescedMap => self.coalesced.dirty_len(),
        }
    }

    pub fn dirty_backlog_counts(&self) -> DirtyBacklogCounts {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => DirtyBacklogCounts {
                dirty_total: self.dirty_mode.len(),
                pending_visible: self.dirty_mode.pending_len(),
                inflight: self.dirty_mode.inflight_len(),
                draining: 0,
            },
            DirtyWriteMode::CoalescedMap => self.coalesced.backlog_counts(),
        }
    }

    #[cfg(feature = "dev-tools")]
    pub fn coalesced_single_write_perf_counters(&self) -> CoalescedSingleWritePerfCounters {
        #[cfg(not(feature = "loom"))]
        return CoalescedSingleWritePerfCounters {
            write_id_ns_total: PERF_COALESCED_SINGLE_WRITE_ID_NS_TOTAL.load(Ordering::Relaxed),
            remove_clean_ns_total: PERF_COALESCED_SINGLE_REMOVE_CLEAN_NS_TOTAL
                .load(Ordering::Relaxed),
            snapshot_check_ns_total: PERF_COALESCED_SINGLE_SNAPSHOT_CHECK_NS_TOTAL
                .load(Ordering::Relaxed),
            active_publish_ns_total: PERF_COALESCED_SINGLE_ACTIVE_PUBLISH_NS_TOTAL
                .load(Ordering::Relaxed),
            snapshot_inflight_check_ns_total:
                PERF_COALESCED_SINGLE_SNAPSHOT_INFLIGHT_CHECK_NS_TOTAL.load(Ordering::Relaxed),
            snapshot_draining_check_ns_total:
                PERF_COALESCED_SINGLE_SNAPSHOT_DRAINING_CHECK_NS_TOTAL.load(Ordering::Relaxed),
            snapshot_inflight_lock_ns_total: PERF_COALESCED_SINGLE_SNAPSHOT_INFLIGHT_LOCK_NS_TOTAL
                .load(Ordering::Relaxed),
            snapshot_inflight_scan_ns_total: PERF_COALESCED_SINGLE_SNAPSHOT_INFLIGHT_SCAN_NS_TOTAL
                .load(Ordering::Relaxed),
            snapshot_inflight_index_lookup_ns_total:
                PERF_COALESCED_SINGLE_SNAPSHOT_INFLIGHT_INDEX_LOOKUP_NS_TOTAL
                    .load(Ordering::Relaxed),
            snapshot_inflight_record_get_ns_total:
                PERF_COALESCED_SINGLE_SNAPSHOT_INFLIGHT_RECORD_GET_NS_TOTAL.load(Ordering::Relaxed),
            snapshot_draining_lock_ns_total: PERF_COALESCED_SINGLE_SNAPSHOT_DRAINING_LOCK_NS_TOTAL
                .load(Ordering::Relaxed),
            snapshot_draining_read_ns_total: PERF_COALESCED_SINGLE_SNAPSHOT_DRAINING_READ_NS_TOTAL
                .load(Ordering::Relaxed),
            active_map_load_ns_total: PERF_COALESCED_SINGLE_ACTIVE_MAP_LOAD_NS_TOTAL
                .load(Ordering::Relaxed),
            active_entry_publish_ns_total: PERF_COALESCED_SINGLE_ACTIVE_ENTRY_PUBLISH_NS_TOTAL
                .load(Ordering::Relaxed),
            active_entry_lookup_ns_total: PERF_COALESCED_SINGLE_ACTIVE_ENTRY_LOOKUP_NS_TOTAL
                .load(Ordering::Relaxed),
            active_entry_occupied_ns_total: PERF_COALESCED_SINGLE_ACTIVE_ENTRY_OCCUPIED_NS_TOTAL
                .load(Ordering::Relaxed),
            active_entry_vacant_ns_total: PERF_COALESCED_SINGLE_ACTIVE_ENTRY_VACANT_NS_TOTAL
                .load(Ordering::Relaxed),
            active_entry_vacant_box_ns_total:
                PERF_COALESCED_SINGLE_ACTIVE_ENTRY_VACANT_BOX_NS_TOTAL.load(Ordering::Relaxed),
            active_entry_vacant_insert_ns_total:
                PERF_COALESCED_SINGLE_ACTIVE_ENTRY_VACANT_INSERT_NS_TOTAL.load(Ordering::Relaxed),
            dirty_count_add_ns_total: PERF_COALESCED_SINGLE_DIRTY_COUNT_ADD_NS_TOTAL
                .load(Ordering::Relaxed),
        };

        #[cfg(feature = "loom")]
        CoalescedSingleWritePerfCounters::default()
    }

    pub fn clean_store_len(&self) -> usize {
        self.clean_count.load(Ordering::Relaxed)
    }

    fn strict_engine(&self) -> StrictEngine<'_, K, V, H> {
        StrictEngine { map: self }
    }

    fn coalesced_engine(&self) -> CoalescedEngine<'_, K, V, H> {
        CoalescedEngine { map: self }
    }

    #[cfg(not(feature = "loom"))]
    fn strict_arc_engine(self: &Arc<Self>) -> StrictArcEngine<K, V, H>
    where
        K: Send + Sync + 'static,
        V: Send + Sync + 'static,
        H: Send + Sync + 'static,
    {
        StrictArcEngine { map: self.clone() }
    }

    #[cfg(not(feature = "loom"))]
    fn coalesced_arc_engine(self: &Arc<Self>) -> CoalescedArcEngine<K, V, H>
    where
        K: Send + Sync + 'static,
        V: Send + Sync + 'static,
        H: Send + Sync + 'static,
    {
        CoalescedArcEngine { map: self.clone() }
    }

    pub fn low_level(&self) -> LowLevelMap<'_, K, V, H> {
        LowLevelMap { map: self }
    }

    pub fn contains<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.read(key, |_, _, _| ()).is_some()
    }

    fn visible_ref<Q>(&self, key: &Q) -> Option<VisibleRef>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => self.strict_engine().visible_ref(key),
            DirtyWriteMode::CoalescedMap => self.coalesced_engine().visible_ref(key),
        }
    }

    pub fn get_cloned<Q>(&self, key: &Q) -> Option<(V, EntryState)>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
        V: Clone,
    {
        self.read(key, |_, value, state| (value.clone(), state))
    }

    fn get_cloned_full<Q>(&self, key: &Q) -> Option<(V, EntryState, VisibleRef)>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
        V: Clone,
    {
        self.read_full(key, |_, value, state, visible| {
            (value.clone(), state, visible)
        })
    }

    pub fn read<Q, R>(&self, key: &Q, reader: impl FnOnce(&K, &V, EntryState) -> R) -> Option<R>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.read_full(key, |stored_key, value, state, _visible| {
            reader(stored_key, value, state)
        })
    }

    fn read_full<Q, R>(
        &self,
        key: &Q,
        reader: impl FnOnce(&K, &V, EntryState, VisibleRef) -> R,
    ) -> Option<R>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => self.strict_engine().read_full(key, reader),
            DirtyWriteMode::CoalescedMap => self.coalesced_engine().read_full(key, reader),
        }
    }

    pub fn for_each_prefix(
        &self,
        prefix: &[u8],
        mut reader: impl FnMut(&K, &V, EntryState),
        limit: usize,
    ) -> usize
    where
        K: Borrow<[u8]>,
    {
        if limit == 0 {
            return 0;
        }
        let keys = self.snapshot_prefix_keys_sorted(prefix, limit);
        let mut matched = 0_usize;
        for key in keys {
            if self.read(key.borrow(), |k, v, s| reader(k, v, s)).is_some() {
                matched += 1;
            }
        }
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
        let keys = self.snapshot_prefix_keys_sorted(prefix, limit);
        let mut matched = 0_usize;
        for key in keys {
            if self.read(key.borrow(), |k, _, _| reader(k)).is_some() {
                matched += 1;
            }
        }
        matched
    }

    pub fn list_prefix<R>(
        &self,
        prefix: &[u8],
        mut reader: impl FnMut(&K, &V, EntryState) -> R,
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
            |key, value, state| {
                out.push(reader(key, value, state));
            },
            limit,
        );
        out
    }

    pub fn put(&self, key: K, value: V) {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => self.strict_engine().put(key, value),
            DirtyWriteMode::CoalescedMap => self.coalesced_engine().put(key, value),
        }
    }

    pub fn put_batch(&self, entries: Vec<(K, V)>) -> usize {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => self.strict_engine().put_batch(entries),
            DirtyWriteMode::CoalescedMap => self.coalesced_engine().put_batch(entries),
        }
    }

    /// Materialize a stable sorted snapshot of currently visible matching keys
    /// and pass it to the caller.
    ///
    /// This is the prefix/list read boundary: it does not flush dirty data.
    pub fn with_prefix_snapshot<R>(
        &self,
        prefix: &[u8],
        limit: usize,
        reader: impl FnOnce(&[K]) -> R,
    ) -> R
    where
        K: Borrow<[u8]>,
    {
        let keys = self.snapshot_prefix_keys_sorted(prefix, limit);
        reader(keys.as_slice())
    }

    fn snapshot_prefix_keys_sorted(&self, prefix: &[u8], limit: usize) -> Vec<K>
    where
        K: Borrow<[u8]>,
    {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => self
                .strict_engine()
                .snapshot_prefix_keys_sorted(prefix, limit),
            DirtyWriteMode::CoalescedMap => self
                .coalesced_engine()
                .snapshot_prefix_keys_sorted(prefix, limit),
        }
    }

    fn insert_dirty(&self, key: K, value: V) -> WriteId {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => self.strict_engine().insert_dirty(key, value),
            DirtyWriteMode::CoalescedMap => self.coalesced_engine().insert_dirty(key, value),
        }
    }

    fn insert_dirty_batch(&self, entries: Vec<(K, V)>) -> Vec<WriteId> {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => self.strict_engine().insert_dirty_batch(entries),
            DirtyWriteMode::CoalescedMap => self.coalesced_engine().insert_dirty_batch(entries),
        }
    }

    fn insert_dirty_batch_without_ids(&self, entries: Vec<(K, V)>) -> usize {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => {
                self.strict_engine().insert_dirty_batch_without_ids(entries)
            }
            DirtyWriteMode::CoalescedMap => self
                .coalesced_engine()
                .insert_dirty_batch_without_ids(entries),
        }
    }

    fn upsert_dirty(&self, key: K, value: V) -> WriteId {
        self.insert_dirty(key, value)
    }

    pub fn insert_clean_if_absent(&self, key: K, value: V) -> Option<CacheId> {
        let id = self.next_cache.fetch_add(1, Ordering::Relaxed);
        let record = Arc::new(CleanRecord { id, key, value });
        let visible = VisibleValue::Clean(record.clone());
        let _coalesced_publish = match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => None,
            DirtyWriteMode::CoalescedMap => Some(lock(&self.coalesced.active_publish)),
        };
        if _coalesced_publish.is_some()
            && self
                .coalesced
                .read_visible_ref(record.key.borrow())
                .is_some()
        {
            return None;
        }
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

    /// Flush pending dirty data through the common persist callback surface.
    ///
    /// The callback receives a [`PersistBatch`] with key/value access only.
    /// Use [`CacheLogMap::low_level`] only when the caller needs the id-bearing
    /// flush path.
    pub fn with_flush_batch<E>(
        &self,
        limit: usize,
        persist: impl FnOnce(&PersistBatch<K, V>) -> Result<(), E>,
    ) -> Result<usize, E> {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => self.strict_engine().with_flush_batch(limit, persist),
            DirtyWriteMode::CoalescedMap => {
                self.coalesced_engine().with_flush_batch(limit, persist)
            }
        }
    }

    /// Repeatedly flush pending dirty data through the common persist callback
    /// surface until no backlog remains.
    pub fn flush_now<E>(
        &self,
        limit: usize,
        persist: impl FnMut(&PersistBatch<K, V>) -> Result<(), E>,
    ) -> Result<usize, E> {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => self.strict_engine().flush_now(limit, persist),
            DirtyWriteMode::CoalescedMap => self.coalesced_engine().flush_now(limit, persist),
        }
    }

    /// Flush pending dirty data, then run the provided scan.
    ///
    /// This is the persisted-scan boundary for callers that need their scan to
    /// agree with the durable store rather than the overlay alone.
    /// Flush pending dirty data through the common persist callback surface,
    /// then run a scan that must agree with persisted state.
    pub fn with_persisted_scan<R, E>(
        &self,
        flush_limit: usize,
        persist: impl FnMut(&PersistBatch<K, V>) -> Result<(), E>,
        scan: impl FnOnce() -> Result<R, E>,
    ) -> Result<R, E> {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => {
                self.strict_engine()
                    .with_persisted_scan(flush_limit, persist, scan)
            }
            DirtyWriteMode::CoalescedMap => {
                self.coalesced_engine()
                    .with_persisted_scan(flush_limit, persist, scan)
            }
        }
    }

    #[cfg(not(feature = "loom"))]
    pub fn start_background_flush(
        self: &Arc<Self>,
        config: BackgroundFlushConfig,
        persist: impl Fn(&PersistBatch<K, V>) -> Result<(), String> + Send + Sync + 'static,
    ) -> BackgroundFlushHandle
    where
        K: Send + Sync + 'static,
        V: Send + Sync + 'static,
        H: Send + Sync + 'static,
    {
        let service = match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => self.strict_arc_engine().background_flush_service(persist),
            DirtyWriteMode::CoalescedMap => self
                .coalesced_arc_engine()
                .background_flush_service(persist),
        };
        BackgroundFlushHandle::start_with_service(config, service)
    }

    #[cfg(not(feature = "loom"))]
    pub fn start_background_flush_default(
        self: &Arc<Self>,
        persist: impl Fn(&PersistBatch<K, V>) -> Result<(), String> + Send + Sync + 'static,
    ) -> BackgroundFlushHandle
    where
        K: Send + Sync + 'static,
        V: Send + Sync + 'static,
        H: Send + Sync + 'static,
    {
        self.start_background_flush(BackgroundFlushConfig::default(), persist)
    }

    #[cfg(not(feature = "loom"))]
    pub fn start_background_flush_for_batch_size(
        self: &Arc<Self>,
        batch_size: usize,
        persist: impl Fn(&PersistBatch<K, V>) -> Result<(), String> + Send + Sync + 'static,
    ) -> BackgroundFlushHandle
    where
        K: Send + Sync + 'static,
        V: Send + Sync + 'static,
        H: Send + Sync + 'static,
    {
        self.start_background_flush(BackgroundFlushConfig::for_batch_size(batch_size), persist)
    }

    /// Acquire a low-level id-bearing flush batch.
    ///
    /// This is intended for strict/debug callers and explicit flush-control
    /// paths. Common product-facing persistence should use [`with_flush_batch`],
    /// [`flush_now`], or [`with_persisted_scan`] instead.
    fn flush_batch(&self, limit: usize) -> FlushBatch<K, V> {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => self.strict_engine().flush_batch(limit),
            DirtyWriteMode::CoalescedMap => self.coalesced_engine().flush_batch(limit),
        }
    }

    fn wait_flush_work(&self, limit: usize) -> FlushWork<K, V> {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => self.strict_engine().wait_flush_work(limit),
            DirtyWriteMode::CoalescedMap => self.coalesced_engine().wait_flush_work(limit),
        }
    }

    fn signal_force_flush(&self) {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => self.strict_engine().signal_force_flush(),
            DirtyWriteMode::CoalescedMap => self.coalesced_engine().signal_force_flush(),
        }
    }

    fn mark_flushed(&self, batch: &FlushBatch<K, V>) -> usize {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => self.strict_engine().mark_flushed(batch),
            DirtyWriteMode::CoalescedMap => self.coalesced_engine().mark_flushed(batch),
        }
    }

    fn cleanup_stale_visible_matching<Q>(&self, key: &Q, expected: VisibleRef) -> bool
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        if self.dirty_write_mode == DirtyWriteMode::CoalescedMap
            && matches!(expected, VisibleRef::Dirty(_))
            && self
                .coalesced
                .read_visible_ref(key)
                .is_some_and(|current| current == expected)
        {
            return true;
        }
        self.visible
            .remove_if_sync(key, |visible| visible.visible_ref() == expected)
            .is_some()
    }

    fn publish_dirty_record(&self, record: Arc<DirtyRecord<K, V>>) {
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

        // Close the publish-after-flush race window: if flush advanced while we
        // were inserting, remove this stale dirty publication immediately.
        if self.is_strict_flushed(id) {
            let _ = self.visible.remove_if_sync(
                &visible_key,
                |entry| matches!(entry, VisibleValue::Dirty(current) if current.id == id),
            );
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
            return self.publish_coalesced_batch(entries);
        }

        let mut scratch = self.take_coalesced_compact_scratch(entries.len());
        Self::compact_last_write_wins_into(entries, &mut scratch);
        let ids = self.publish_coalesced_batch_from_scratch(&mut scratch.compacted);
        self.return_coalesced_compact_scratch(scratch);
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
            return self.publish_coalesced_batch_without_ids(entries);
        }

        let mut scratch = self.take_coalesced_compact_scratch(entries.len());
        Self::compact_last_write_wins_into(entries, &mut scratch);
        let written = self.publish_coalesced_batch_without_ids_from_scratch(&mut scratch.compacted);
        self.return_coalesced_compact_scratch(scratch);
        written
    }

    fn compact_last_write_wins_into(
        entries: Vec<(K, V)>,
        scratch: &mut CoalescedCompactScratch<K, V>,
    ) {
        // Hash to retained-slot mapping avoids cloning owned keys into the
        // duplicate-detection map while preserving full Eq semantics.
        scratch.ensure_capacity(entries.len());
        scratch.clear();
        for (key, value) in entries {
            let fingerprint = scratch.fingerprint_hasher.hash_one(&key);
            match scratch.latest_slot.entry(fingerprint) {
                std::collections::hash_map::Entry::Occupied(mut occupied) => match occupied
                    .get_mut()
                {
                    DedupeSlots::One(slot) => {
                        if scratch.compacted[*slot].0 == key {
                            scratch.compacted[*slot].1 = value;
                        } else {
                            let next_slot = scratch.compacted.len();
                            let prior_slot = *slot;
                            *occupied.get_mut() = DedupeSlots::Many(vec![prior_slot, next_slot]);
                            scratch.compacted.push((key, value));
                        }
                    }
                    DedupeSlots::Many(slots) => {
                        if let Some(existing_slot) = slots
                            .iter()
                            .copied()
                            .find(|slot| scratch.compacted[*slot].0 == key)
                        {
                            scratch.compacted[existing_slot].1 = value;
                        } else {
                            let next_slot = scratch.compacted.len();
                            slots.push(next_slot);
                            scratch.compacted.push((key, value));
                        }
                    }
                },
                std::collections::hash_map::Entry::Vacant(vacant) => {
                    let next_slot = scratch.compacted.len();
                    vacant.insert(DedupeSlots::One(next_slot));
                    scratch.compacted.push((key, value));
                }
            }
        }
    }

    fn publish_coalesced_active(&self, key: K, id: WriteId, value: V) -> bool {
        let _publish = lock(&self.coalesced.active_publish);
        self.remove_visible_clean_if_present(&key);
        self.coalesced.publish_active_locked(key, id, value)
    }

    fn publish_coalesced_single_write(&self, key: K, value: V) -> WriteId {
        #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
        let write_id_started = coalesced_perf_enabled().then(StdInstant::now);
        let id = self.next_write_id();
        #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
        if let Some(started) = write_id_started {
            PERF_COALESCED_SINGLE_WRITE_ID_NS_TOTAL
                .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }

        #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
        let remove_clean_started = coalesced_perf_enabled().then(StdInstant::now);
        let inserted = self.publish_coalesced_active(key, id, value);
        #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
        if let Some(started) = remove_clean_started {
            PERF_COALESCED_SINGLE_REMOVE_CLEAN_NS_TOTAL
                .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
        let _ = inserted;
        id
    }

    fn publish_coalesced_single_write_without_id(&self, key: K, value: V) {
        #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
        let remove_clean_started = coalesced_perf_enabled().then(StdInstant::now);
        self.coalesced_has_unassigned_ids
            .store(1, Ordering::Release);
        let inserted = self.publish_coalesced_active(key, COALESCED_UNASSIGNED_WRITE_ID, value);
        #[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
        if let Some(started) = remove_clean_started {
            PERF_COALESCED_SINGLE_REMOVE_CLEAN_NS_TOTAL
                .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
        let _ = inserted;
    }

    fn publish_coalesced_batch(&self, entries: Vec<(K, V)>) -> Vec<WriteId> {
        if entries.is_empty() {
            return Vec::new();
        }

        let len = entries.len();
        let id_base = self.reserve_write_ids(len);
        let mut ids = Vec::with_capacity(len);
        for (offset, (key, value)) in entries.into_iter().enumerate() {
            let id = id_base.wrapping_add(offset as u64);
            let _ = self.publish_coalesced_active(key, id, value);
            ids.push(id);
        }
        ids
    }

    fn publish_coalesced_batch_from_scratch(&self, entries: &mut Vec<(K, V)>) -> Vec<WriteId> {
        if entries.is_empty() {
            return Vec::new();
        }

        let len = entries.len();
        let id_base = self.reserve_write_ids(len);
        let mut ids = Vec::with_capacity(len);
        for (offset, (key, value)) in entries.drain(..).enumerate() {
            let id = id_base.wrapping_add(offset as u64);
            let _ = self.publish_coalesced_active(key, id, value);
            ids.push(id);
        }
        ids
    }

    fn publish_coalesced_batch_without_ids(&self, entries: Vec<(K, V)>) -> usize {
        if entries.is_empty() {
            return 0;
        }

        let len = entries.len();
        let id_base = self.reserve_write_ids(len);
        for (offset, (key, value)) in entries.into_iter().enumerate() {
            let id = id_base.wrapping_add(offset as u64);
            let _ = self.publish_coalesced_active(key, id, value);
        }
        len
    }

    fn publish_coalesced_batch_without_ids_from_scratch(&self, entries: &mut Vec<(K, V)>) -> usize {
        if entries.is_empty() {
            return 0;
        }

        let len = entries.len();
        let id_base = self.reserve_write_ids(len);
        for (offset, (key, value)) in entries.drain(..).enumerate() {
            let id = id_base.wrapping_add(offset as u64);
            let _ = self.publish_coalesced_active(key, id, value);
        }
        len
    }

    fn remove_visible_clean_if_present<Q>(&self, key: &Q)
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        if self.clean_count.load(Ordering::Relaxed) == 0 {
            return;
        }
        if self
            .visible
            .remove_if_sync(key, |visible| matches!(visible, VisibleValue::Clean(_)))
            .is_some()
        {
            self.clean_count.fetch_sub(1, Ordering::Relaxed);
        }
    }

    fn next_write_id(&self) -> WriteId {
        self.next_dirty_write.fetch_add(1, Ordering::Relaxed)
    }

    fn reserve_write_ids(&self, count: usize) -> WriteId {
        self.next_dirty_write
            .fetch_add(count as u64, Ordering::Relaxed)
    }

    fn take_coalesced_compact_scratch(&self, capacity: usize) -> CoalescedCompactScratch<K, V> {
        self.coalesced.take_compact_scratch(capacity)
    }

    fn return_coalesced_compact_scratch(&self, scratch: CoalescedCompactScratch<K, V>) {
        self.coalesced.return_compact_scratch(scratch);
    }

    fn is_strict_flushed(&self, id: WriteId) -> bool {
        let flushed_upto = self.strict_flushed_upto.load(Ordering::SeqCst);
        flushed_upto != u64::MAX && id <= flushed_upto
    }

    fn advance_strict_flushed_upto(&self, id: WriteId) {
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

    fn flush_batch_coalesced(&self, limit: usize) -> FlushBatch<K, V> {
        if limit == 0 {
            return FlushBatch::new(Vec::new());
        }

        let mut inflight = lock(&self.coalesced.inflight);
        if let Some(current) = inflight.as_ref() {
            return current.batch.clone();
        }

        let _publish = lock(&self.coalesced.active_publish);
        self.coalesced.ensure_draining_snapshot_locked();
        let mut entries = self.coalesced.begin_snapshot_drain_batch(limit);
        if entries.is_empty() {
            return FlushBatch::new(Vec::new());
        }
        if self.coalesced_has_unassigned_ids.load(Ordering::Acquire) != 0 {
            let mut unassigned = 0_u64;
            for record in &entries {
                if record.id == COALESCED_UNASSIGNED_WRITE_ID {
                    unassigned += 1;
                }
            }
            if unassigned > 0 {
                let id_base = self.reserve_write_ids(unassigned as usize);
                let mut offset = 0_u64;
                for record in &mut entries {
                    if record.id == COALESCED_UNASSIGNED_WRITE_ID {
                        record.id = id_base.wrapping_add(offset);
                        offset = offset.wrapping_add(1);
                    }
                }
            }
        }
        let batch = FlushBatch::new_owned(entries);

        let current = Arc::new(CoalescedInflight {
            batch: batch.clone(),
            index: CoalescedOverlay::<K, V, H>::build_inflight_index(&batch),
        });
        self.coalesced.inflight_shared.store(Some(current.clone()));
        *inflight = Some(current);
        batch
    }

    fn mark_flushed_coalesced(&self, batch: &FlushBatch<K, V>) -> usize {
        self.coalesced.mark_flushed_if_current(batch)
    }

    fn read_coalesced_inflight<Q, R>(
        &self,
        key: &Q,
        reader: impl FnOnce(&DirtyRecord<K, V>) -> R,
    ) -> Option<R>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.coalesced.read_inflight(key, reader)
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

impl<V, H> CacheLogMap<Vec<u8>, V, H>
where
    H: BuildHasher + Clone,
{
    fn borrowed_batch_has_duplicates(entries: &[(&[u8], &[u8])]) -> bool {
        if entries.len() < 2 {
            return false;
        }

        let mut ordered_unique = true;
        for window in entries.windows(2) {
            if window[0].0 >= window[1].0 {
                ordered_unique = false;
                break;
            }
        }
        if ordered_unique {
            return false;
        }

        let mut seen = AHashSet::with_capacity(entries.len());
        for (key, _) in entries {
            if !seen.insert(*key) {
                return true;
            }
        }
        false
    }

    fn publish_coalesced_borrowed_batch_without_ids<'a, F>(
        &self,
        entries: Vec<(&'a [u8], &'a [u8])>,
        mut make_value: F,
    ) -> usize
    where
        F: FnMut(&'a [u8]) -> V,
    {
        if entries.is_empty() {
            return 0;
        }

        let len = entries.len();
        let id_base = self.reserve_write_ids(len);
        let _publish = lock(&self.coalesced.active_publish);
        for (offset, (key, raw_value)) in entries.into_iter().enumerate() {
            let id = id_base.wrapping_add(offset as u64);
            self.remove_visible_clean_if_present(key);
            let _ = self
                .coalesced
                .update_or_insert(key, key.to_vec(), id, make_value(raw_value));
        }
        len
    }

    fn compact_and_publish_coalesced_borrowed_batch_without_ids<'a, F>(
        &self,
        entries: Vec<(&'a [u8], &'a [u8])>,
        mut make_value: F,
    ) -> usize
    where
        F: FnMut(&'a [u8]) -> V,
    {
        if entries.is_empty() {
            return 0;
        }

        borrowed_keys::with_borrowed_coalesced_keys(|scratch_keys| {
            let mut latest_slot: AHashMap<&'a [u8], usize> = AHashMap::with_capacity(entries.len());
            let mut values: Vec<Option<V>> = Vec::with_capacity(entries.len());
            let mut used = 0_usize;

            for (key, raw_value) in entries {
                if let Some(&slot) = latest_slot.get(key) {
                    values[slot] = Some(make_value(raw_value));
                    continue;
                }

                latest_slot.insert(key, used);
                if used == scratch_keys.len() {
                    scratch_keys.push(key.to_vec());
                } else {
                    let scratch_key = &mut scratch_keys[used];
                    scratch_key.clear();
                    scratch_key.extend_from_slice(key);
                }

                if used == values.len() {
                    values.push(Some(make_value(raw_value)));
                } else {
                    values[used] = Some(make_value(raw_value));
                }
                used += 1;
            }

            let id_base = self.reserve_write_ids(used);
            let _publish = lock(&self.coalesced.active_publish);
            for slot in 0..used {
                let id = id_base.wrapping_add(slot as u64);
                let key = scratch_keys[slot].as_slice();
                self.remove_visible_clean_if_present(key);
                let value = values[slot]
                    .take()
                    .expect("borrowed compacted value should exist");
                let _ = self
                    .coalesced
                    .update_or_insert(key, scratch_keys[slot].clone(), id, value);
            }

            used
        })
    }
}

impl<H> CacheLogMap<Vec<u8>, Vec<u8>, H>
where
    H: BuildHasher + Clone,
{
    fn insert_dirty_borrowed(&self, key: &[u8], value: &[u8]) -> WriteId {
        self.insert_dirty(key.to_vec(), value.to_vec())
    }

    fn insert_dirty_batch_borrowed_without_ids<'a, I>(&self, entries: I) -> usize
    where
        I: IntoIterator<Item = (&'a [u8], &'a [u8])>,
    {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => {
                let records = self.dirty_mode.append_batch_borrowed(entries);
                if records.is_empty() {
                    return 0;
                }

                let _reserved = self.visible.reserve(records.len());
                let len = records.len();
                for record in records {
                    self.publish_dirty_record(record);
                }
                len
            }
            DirtyWriteMode::CoalescedMap => {
                let entries: Vec<_> = entries.into_iter().collect();
                if !Self::borrowed_batch_has_duplicates(&entries) {
                    return self.publish_coalesced_borrowed_batch_without_ids(entries, |value| {
                        value.to_vec()
                    });
                }

                self.compact_and_publish_coalesced_borrowed_batch_without_ids(entries, |value| {
                    value.to_vec()
                })
            }
        }
    }
}

#[cfg(any(test, feature = "dev-tools"))]
impl<H> CacheLogMap<Vec<u8>, Arc<Vec<u8>>, H>
where
    H: BuildHasher + Clone,
{
    fn insert_dirty_borrowed(&self, key: &[u8], value: &[u8]) -> WriteId {
        self.insert_dirty(key.to_vec(), Arc::new(value.to_vec()))
    }

    fn insert_dirty_batch_borrowed_without_ids<'a, I>(&self, entries: I) -> usize
    where
        I: IntoIterator<Item = (&'a [u8], &'a [u8])>,
    {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => {
                let records = self.dirty_mode.append_batch_borrowed(entries);
                if records.is_empty() {
                    return 0;
                }

                let _reserved = self.visible.reserve(records.len());
                let len = records.len();
                for record in records {
                    self.publish_dirty_record(record);
                }
                len
            }
            DirtyWriteMode::CoalescedMap => {
                let entries: Vec<_> = entries.into_iter().collect();
                if !Self::borrowed_batch_has_duplicates(&entries) {
                    return self.publish_coalesced_borrowed_batch_without_ids(entries, |value| {
                        Arc::new(value.to_vec())
                    });
                }

                self.compact_and_publish_coalesced_borrowed_batch_without_ids(entries, |value| {
                    Arc::new(value.to_vec())
                })
            }
        }
    }
}

#[cfg(all(not(feature = "loom"), any(test, feature = "dev-tools")))]
impl<H> CacheLogMap<Vec<u8>, Arc<[u8]>, H>
where
    H: BuildHasher + Clone,
{
    fn insert_dirty_borrowed(&self, key: &[u8], value: &[u8]) -> WriteId {
        self.insert_dirty(key.to_vec(), Arc::<[u8]>::from(value.to_vec()))
    }

    fn insert_dirty_preowned(&self, key: Vec<u8>, value: Arc<[u8]>) -> WriteId {
        self.insert_dirty(key, value)
    }

    fn insert_dirty_batch_preowned_without_ids(&self, entries: Vec<(Vec<u8>, Arc<[u8]>)>) -> usize {
        self.insert_dirty_batch_without_ids(entries)
    }

    fn insert_dirty_batch_borrowed_without_ids<'a, I>(&self, entries: I) -> usize
    where
        I: IntoIterator<Item = (&'a [u8], &'a [u8])>,
    {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => {
                let records = self.dirty_mode.append_batch_borrowed(entries);
                if records.is_empty() {
                    return 0;
                }

                let _reserved = self.visible.reserve(records.len());
                let len = records.len();
                for record in records {
                    self.publish_dirty_record(record);
                }
                len
            }
            DirtyWriteMode::CoalescedMap => {
                let entries: Vec<_> = entries.into_iter().collect();
                if !Self::borrowed_batch_has_duplicates(&entries) {
                    return self.publish_coalesced_borrowed_batch_without_ids(entries, |value| {
                        Arc::<[u8]>::from(value.to_vec())
                    });
                }

                self.compact_and_publish_coalesced_borrowed_batch_without_ids(entries, |value| {
                    Arc::<[u8]>::from(value.to_vec())
                })
            }
        }
    }
}

#[cfg(any(test, feature = "dev-tools"))]
impl<H> CacheLogMap<Vec<u8>, Bytes, H>
where
    H: BuildHasher + Clone,
{
    fn insert_dirty_borrowed(&self, key: &[u8], value: &[u8]) -> WriteId {
        self.insert_dirty(key.to_vec(), bytes_from_borrowed(value))
    }

    fn insert_dirty_owned_key_borrowed_value(&self, key: Vec<u8>, value: &[u8]) -> WriteId {
        self.insert_dirty(key, bytes_from_borrowed(value))
    }

    fn insert_dirty_preowned(&self, key: Vec<u8>, value: Bytes) -> WriteId {
        self.insert_dirty(key, value)
    }

    fn insert_dirty_batch_owned_keys_borrowed_values_without_ids<'a, I>(&self, entries: I) -> usize
    where
        I: IntoIterator<Item = (Vec<u8>, &'a [u8])>,
    {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => {
                let records = self.dirty_mode.append_batch(
                    entries
                        .into_iter()
                        .map(|(key, value)| (key, bytes_from_borrowed(value)))
                        .collect(),
                );
                if records.is_empty() {
                    return 0;
                }

                let _reserved = self.visible.reserve(records.len());
                let len = records.len();
                for record in records {
                    self.publish_dirty_record(record);
                }
                len
            }
            DirtyWriteMode::CoalescedMap => {
                let owned = entries
                    .into_iter()
                    .map(|(key, value)| (key, bytes_from_borrowed(value)))
                    .collect();
                self.insert_dirty_batch_coalesced_without_ids(owned)
            }
        }
    }

    fn insert_dirty_batch_preowned_without_ids(&self, entries: Vec<(Vec<u8>, Bytes)>) -> usize {
        self.insert_dirty_batch_without_ids(entries)
    }

    fn insert_dirty_batch_borrowed_without_ids<'a, I>(&self, entries: I) -> usize
    where
        I: IntoIterator<Item = (&'a [u8], &'a [u8])>,
    {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => {
                let records = self.dirty_mode.append_batch_borrowed(entries);
                if records.is_empty() {
                    return 0;
                }

                let _reserved = self.visible.reserve(records.len());
                let len = records.len();
                for record in records {
                    self.publish_dirty_record(record);
                }
                len
            }
            DirtyWriteMode::CoalescedMap => {
                let entries: Vec<_> = entries.into_iter().collect();
                if !Self::borrowed_batch_has_duplicates(&entries) {
                    return self.publish_coalesced_borrowed_batch_without_ids(
                        entries,
                        bytes_from_borrowed,
                    );
                }

                self.compact_and_publish_coalesced_borrowed_batch_without_ids(
                    entries,
                    bytes_from_borrowed,
                )
            }
        }
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
impl<H> LowLevelMap<'_, Vec<u8>, Bytes, H>
where
    H: BuildHasher + Clone,
{
    pub fn insert_dirty_borrowed(&self, key: &[u8], value: &[u8]) -> WriteId {
        self.map.insert_dirty_borrowed(key, value)
    }

    pub fn insert_dirty_owned_key_borrowed_value(&self, key: Vec<u8>, value: &[u8]) -> WriteId {
        self.map.insert_dirty_owned_key_borrowed_value(key, value)
    }

    pub fn insert_dirty_preowned(&self, key: Vec<u8>, value: Bytes) -> WriteId {
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

    pub fn insert_dirty_batch_preowned_without_ids(&self, entries: Vec<(Vec<u8>, Bytes)>) -> usize {
        self.map.insert_dirty_batch_preowned_without_ids(entries)
    }

    pub fn insert_dirty_batch_borrowed_without_ids<'a, I>(&self, entries: I) -> usize
    where
        I: IntoIterator<Item = (&'a [u8], &'a [u8])>,
    {
        self.map.insert_dirty_batch_borrowed_without_ids(entries)
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
    fn debug_snapshot(&self) -> DebugSnapshot<K, V> {
        let mut visible = Vec::new();
        if self.dirty_write_mode == DirtyWriteMode::CoalescedMap {
            self.coalesced.for_each_active_key(|key| {
                let record = self
                    .coalesced
                    .read_active(key, |_, record| DebugRecord {
                        id: record.id,
                        key: key.clone(),
                        value: record.value.clone(),
                    })
                    .expect("active coalesced key vanished during debug snapshot");
                visible.push((key.clone(), DebugVisible::Dirty(record)));
            });
            self.coalesced.with_inflight(|inflight| {
                if let Some(inflight) = inflight {
                    for record in inflight.batch.iter() {
                        if !visible
                            .iter()
                            .any(|(existing_key, _)| existing_key == &record.key)
                        {
                            visible.push((
                                record.key.clone(),
                                DebugVisible::Dirty(DebugRecord {
                                    id: record.id,
                                    key: record.key.clone(),
                                    value: record.value.clone(),
                                }),
                            ));
                        }
                    }
                }
            });
            self.coalesced.for_each_draining_key(|key| {
                if visible.iter().any(|(existing_key, _)| existing_key == key) {
                    return;
                }
                let record = self
                    .coalesced
                    .read_draining(key, |_, record| DebugRecord {
                        id: record.id,
                        key: key.clone(),
                        value: record.value.clone(),
                    })
                    .expect("draining coalesced key vanished during debug snapshot");
                visible.push((key.clone(), DebugVisible::Dirty(record)));
            });
        }
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
                let inflight_records = self.coalesced.with_inflight(|inflight| {
                    inflight
                        .map(|inflight| {
                            inflight
                                .batch
                                .iter()
                                .map(|record| DebugRecord {
                                    id: record.id,
                                    key: record.key.clone(),
                                    value: record.value.clone(),
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                });
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

#[cfg(any(test, feature = "loom"))]
impl<K, V, H> LowLevelMap<'_, K, V, H>
where
    K: Clone + Eq + Hash + Ord,
    V: Clone,
    H: BuildHasher + Clone,
{
    #[doc(hidden)]
    pub fn debug_snapshot(&self) -> DebugSnapshot<K, V> {
        self.map.debug_snapshot()
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
                let actual_triplet = self.live.read_full(&key, |_, value, state, visible| {
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
