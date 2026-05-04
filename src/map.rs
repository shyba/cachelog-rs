#[cfg(any(test, feature = "dev-tools"))]
use bytes::Bytes;
use scc::HashMap as ConcurrentHashMap;
use scc::hash_map::Entry as MapEntry;
use std::borrow::Borrow;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hash};

#[cfg(not(feature = "loom"))]
use crate::background_flush::{BackgroundFlushConfig, BackgroundFlushHandle};
#[cfg(any(test, feature = "dev-tools"))]
use crate::bytes_pooling::bytes_from_borrowed;
use crate::dirty_mode::{DirtyMode, OrderedFifoDirty};
use crate::entry::{
    CacheId, CleanRecord, EntryState, FlushBatch, PersistBatch, VisibleRef, WriteId,
};
#[cfg(feature = "dev-tools")]
use crate::perf::CoalescedSingleWritePerfCounters;
#[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
use crate::perf::*;
use crate::sync::{
    Arc, AtomicU64, AtomicUsize, BoundedQueue, Ordering, SharedArc, SharedOptionArc, lock,
    new_mutex,
};

pub(crate) mod api;
pub(crate) mod coalesced;
#[cfg(any(test, feature = "loom"))]
pub(crate) mod debug;
pub(crate) mod engine;
pub(crate) mod low_level;
pub(crate) mod strict;
pub(crate) mod types;
pub(crate) use coalesced::{CoalescedCompactScratch, CoalescedOverlay};
#[cfg(all(test, not(feature = "loom")))]
pub(crate) use debug::DebugVisible;
#[cfg(feature = "loom")]
pub use debug::{DebugRecord, DebugSnapshot, DebugVisible};
#[cfg(not(feature = "loom"))]
pub(crate) use engine::{CoalescedArcEngine, StrictArcEngine};
pub(crate) use engine::{CoalescedEngine, StrictEngine};
pub use types::{CacheLogConfig, DirtyBacklogCounts, DirtyWriteMode, FlushWork};
pub(crate) use types::{CoalescedInflight, CoalescedRecord, VisibleValue};

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

impl<V, H> CacheLogMap<Vec<u8>, V, H> where H: BuildHasher + Clone {}

impl<H> CacheLogMap<Vec<u8>, Vec<u8>, H>
where
    H: BuildHasher + Clone,
{
    fn insert_dirty_borrowed(&self, key: &[u8], value: &[u8]) -> WriteId {
        self.insert_dirty_borrowed_with(key, value, |value| value.to_vec())
    }

    fn insert_dirty_batch_borrowed_without_ids<'a, I>(&self, entries: I) -> usize
    where
        I: IntoIterator<Item = (&'a [u8], &'a [u8])>,
    {
        self.insert_dirty_batch_borrowed_without_ids_with(entries, |value| value.to_vec())
    }
}

#[cfg(any(test, feature = "dev-tools"))]
impl<H> CacheLogMap<Vec<u8>, Arc<Vec<u8>>, H>
where
    H: BuildHasher + Clone,
{
    fn insert_dirty_borrowed(&self, key: &[u8], value: &[u8]) -> WriteId {
        self.insert_dirty_borrowed_with(key, value, |value| Arc::new(value.to_vec()))
    }

    fn insert_dirty_batch_borrowed_without_ids<'a, I>(&self, entries: I) -> usize
    where
        I: IntoIterator<Item = (&'a [u8], &'a [u8])>,
    {
        self.insert_dirty_batch_borrowed_without_ids_with(entries, |value| Arc::new(value.to_vec()))
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
        self.insert_dirty_batch_borrowed_without_ids_with(entries, |value| {
            Arc::<[u8]>::from(value.to_vec())
        })
    }
}

#[cfg(any(test, feature = "dev-tools"))]
impl<H> CacheLogMap<Vec<u8>, Bytes, H>
where
    H: BuildHasher + Clone,
{
    fn insert_dirty_borrowed(&self, key: &[u8], value: &[u8]) -> WriteId {
        self.insert_dirty_borrowed_with(key, value, bytes_from_borrowed)
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
        self.insert_dirty_batch_borrowed_without_ids_with(entries, bytes_from_borrowed)
    }
}

impl<V, H> CacheLogMap<Vec<u8>, V, H>
where
    H: BuildHasher + Clone,
{
    fn insert_dirty_borrowed_with<F>(&self, key: &[u8], value: &[u8], make_value: F) -> WriteId
    where
        F: FnOnce(&[u8]) -> V,
    {
        self.insert_dirty(key.to_vec(), make_value(value))
    }

    fn insert_dirty_batch_borrowed_without_ids_with<'a, I, F>(
        &self,
        entries: I,
        mut make_value: F,
    ) -> usize
    where
        I: IntoIterator<Item = (&'a [u8], &'a [u8])>,
        F: FnMut(&'a [u8]) -> V,
    {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => {
                let records = self.dirty_mode.append_batch(
                    entries
                        .into_iter()
                        .map(|(key, value)| (key.to_vec(), make_value(value)))
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
                let entries: Vec<_> = entries.into_iter().collect();
                if !Self::borrowed_batch_has_duplicates(&entries) {
                    return self.publish_coalesced_borrowed_batch_without_ids(entries, make_value);
                }

                self.compact_and_publish_coalesced_borrowed_batch_without_ids(entries, make_value)
            }
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
