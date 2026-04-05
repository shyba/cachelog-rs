use std::collections::VecDeque;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hash};
use crate::sync::{Arc, AtomicU64, AtomicUsize, BoundedQueue, Mutex, Ordering, lock, new_mutex};
use scc::HashMap as ConcurrentHashMap;
use scc::hash_map::Entry as MapEntry;

use crate::entry::{
    CacheId, CleanRecord, DirtyRecord, EntryState, FlushBatch, VisibleRef, WriteId,
};

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

struct DirtyLog<K, V> {
    next_id: WriteId,
    entries: VecDeque<Arc<DirtyRecord<K, V>>>,
    inflight: Option<FlushBatch<K, V>>,
}

impl<K, V> DirtyLog<K, V> {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            next_id: 0,
            entries: VecDeque::with_capacity(capacity),
            inflight: None,
        }
    }

    fn append(&mut self, key: K, value: V) -> Arc<DirtyRecord<K, V>> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let record = Arc::new(DirtyRecord { id, key, value });
        self.entries.push_back(record.clone());
        record
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
    visible: ConcurrentHashMap<K, VisibleValue<K, V>, H>,
    dirty_log: Mutex<DirtyLog<K, V>>,
    clean_fifo: BoundedQueue<(K, CacheId)>,
    clean_count: AtomicUsize,
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
            visible: ConcurrentHashMap::with_capacity_and_hasher(
                config.visible_capacity,
                build_hasher,
            ),
            dirty_log: new_mutex(DirtyLog::with_capacity(config.dirty_log_capacity)),
            clean_fifo: BoundedQueue::new(config.clean_capacity),
            clean_count: AtomicUsize::new(0),
            next_cache: AtomicU64::new(0),
        }
    }

    pub fn visible_len(&self) -> usize {
        self.visible.len()
    }

    pub fn dirty_log_len(&self) -> usize {
        lock(&self.dirty_log).len()
    }

    pub fn clean_store_len(&self) -> usize {
        self.clean_count.load(Ordering::Relaxed)
    }

    pub fn contains(&self, key: &K) -> bool {
        self.read(key, |_, _, _, _| ()).is_some()
    }

    pub fn visible_ref(&self, key: &K) -> Option<VisibleRef> {
        self.visible
            .read_sync(key, |_, visible| visible.visible_ref())
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

    pub fn insert_dirty(&self, key: K, value: V) -> WriteId {
        let record = lock(&self.dirty_log).append(key, value);
        let id = record.id;
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
        id
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
            MapEntry::Occupied(occupied) => matches!(occupied.get(), VisibleValue::Clean(_))
                .then_some(false)
                .unwrap_or(false),
        };
        if inserted {
            self.enqueue_clean(record.key.clone(), id);
            Some(id)
        } else {
            None
        }
    }

    pub fn evict_clean(&self, key: &K) -> bool {
        self.visible
            .remove_if_sync(key, |visible| matches!(visible, VisibleValue::Clean(_)))
            .map(|_| {
                self.clean_count.fetch_sub(1, Ordering::Relaxed);
                true
            })
            .unwrap_or(false)
    }

    pub fn cleanup_stale_visible(&self, key: &K) -> bool {
        let Some(visible_ref) = self.visible_ref(key) else {
            return false;
        };
        self.cleanup_stale_visible_matching(key, visible_ref)
    }

    pub fn flush_batch(&self, limit: usize) -> FlushBatch<K, V> {
        lock(&self.dirty_log).pending_batch(limit)
    }

    pub fn mark_flushed(&self, batch: &FlushBatch<K, V>) -> usize {
        let flushed_records = batch.entries.iter().map(Arc::clone).collect::<Vec<_>>();
        let mut dirty_log = lock(&self.dirty_log);
        let marked = dirty_log.mark_flushed(batch);
        drop(dirty_log);
        if marked == 0 {
            return 0;
        }
        for record in flushed_records {
            let _ = self.visible.remove_if_sync(&record.key, |visible| {
                matches!(visible, VisibleValue::Dirty(current) if Arc::ptr_eq(current, &record))
            });
        }
        marked
    }

    fn cleanup_stale_visible_matching(&self, key: &K, expected: VisibleRef) -> bool {
        self.visible
            .remove_if_sync(key, |visible| visible.visible_ref() == expected)
            .is_some()
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

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
struct TestingRecord<K, V> {
    id: u64,
    key: K,
    value: V,
}

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
enum TestingVisible<K, V> {
    Dirty(TestingRecord<K, V>),
    Clean(TestingRecord<K, V>),
}

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
struct TestingSnapshot<K, V> {
    visible: Vec<(K, TestingVisible<K, V>)>,
    dirty_pending: Vec<TestingRecord<K, V>>,
    dirty_inflight: Vec<TestingRecord<K, V>>,
    next_write: u64,
    next_cache: u64,
    clean_count: usize,
}

#[cfg(test)]
impl<K, V, H> CacheLogMap<K, V, H>
where
    K: Clone + Eq + Hash + Ord,
    V: Clone,
    H: BuildHasher + Clone,
{
    fn testing_snapshot(&self) -> TestingSnapshot<K, V> {
        let mut visible = Vec::new();
        self.visible.iter_sync(|key, value| {
            let projected = match value {
                VisibleValue::Dirty(record) => TestingVisible::Dirty(TestingRecord {
                    id: record.id,
                    key: record.key.clone(),
                    value: record.value.clone(),
                }),
                VisibleValue::Clean(record) => TestingVisible::Clean(TestingRecord {
                    id: record.id,
                    key: record.key.clone(),
                    value: record.value.clone(),
                }),
            };
            visible.push((key.clone(), projected));
            true
        });
        visible.sort_by(|left, right| left.0.cmp(&right.0));

        let dirty_log = lock(&self.dirty_log);
        let dirty_pending = dirty_log
            .entries
            .iter()
            .map(|record| TestingRecord {
                id: record.id,
                key: record.key.clone(),
                value: record.value.clone(),
            })
            .collect();
        let dirty_inflight = dirty_log
            .inflight
            .as_ref()
            .map(|batch| {
                batch.entries
                    .iter()
                    .map(|record| TestingRecord {
                        id: record.id,
                        key: record.key.clone(),
                        value: record.value.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        TestingSnapshot {
            visible,
            dirty_pending,
            dirty_inflight,
            next_write: dirty_log.next_id,
            next_cache: self.next_cache.load(Ordering::Relaxed),
            clean_count: self.clean_count.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
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
            let ids = batch.iter().map(|record| to_core_id(record.id)).collect::<Vec<_>>();
            if self.inflight.is_empty() {
                let model_queue = self.model.dirty_q.clone();
                assert_eq!(ids, model_queue.iter().copied().take(limit).collect::<Vec<_>>());
            } else {
                assert_eq!(ids, self.inflight);
            }
            self.inflight = ids.clone();
            self.assert_matches_model();
            ids
        }

        fn mark_flushed(&mut self, limit: usize) -> Vec<usize> {
            let batch = self.live.flush_batch(limit);
            let ids = batch.iter().map(|record| to_core_id(record.id)).collect::<Vec<_>>();
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
                assert_eq!(actual_visible, expected_visible, "visible_ref mismatch for key {key}");

                let expected_triplet = match self.model.visible[key] {
                    ModelVisibleRef::None => None,
                    ModelVisibleRef::Dirty(id) => {
                        let record = &self.model.write_store[id];
                        Some((record.value, EntryState::Dirty, ComparableVisibleRef::Dirty(id)))
                    }
                    ModelVisibleRef::Clean(id) => {
                        let record = &self.model.cache_store[id];
                        Some((record.value, EntryState::Clean, ComparableVisibleRef::Clean(id)))
                    }
                };
                let actual_triplet = self.live.read(&key, |_, value, state, visible| {
                    let visible = match visible {
                        VisibleRef::Dirty(id) => ComparableVisibleRef::Dirty(to_core_id(id)),
                        VisibleRef::Clean(id) => ComparableVisibleRef::Clean(to_core_id(id)),
                    };
                    (*value, state, visible)
                });
                assert_eq!(actual_triplet, expected_triplet, "read mismatch for key {key}");
                assert_eq!(self.live.contains(&key), expected_triplet.is_some(), "contains mismatch for key {key}");
            }
        }

        fn assert_matches_model(&self) {
            let snapshot = self.live.testing_snapshot();
            let comparable = ComparableState::from(&self.model);

            let visible = snapshot
                .visible
                .iter()
                .map(|(key, visible)| {
                    let visible = match visible {
                        TestingVisible::Dirty(record) => {
                            ComparableVisibleRef::Dirty(to_core_id(record.id))
                        }
                        TestingVisible::Clean(record) => {
                            ComparableVisibleRef::Clean(to_core_id(record.id))
                        }
                    };
                    (*key, visible)
                })
                .collect::<BTreeMap<_, _>>();
            assert_eq!(visible, comparable.visible);

            let mut write_store = BTreeMap::new();
            for (_, visible) in &snapshot.visible {
                if let TestingVisible::Dirty(record) = visible {
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
                    TestingVisible::Clean(record) => Some((
                        to_core_id(record.id),
                        ComparableCleanRecord {
                            id: to_core_id(record.id),
                            key: record.key,
                            value: record.value,
                        },
                    )),
                    TestingVisible::Dirty(_) => None,
                })
                .collect::<BTreeMap<_, _>>();
            assert_eq!(cache_store, comparable.cache_store);
            assert_eq!(snapshot.next_write, lock(&self.live.dirty_log).next_id);
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
