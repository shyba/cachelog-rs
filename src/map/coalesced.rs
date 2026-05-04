use super::{
    CacheLogMap, CoalescedInflight, CoalescedRecord, DirtyBacklogCounts,
    types::{CoalescedActiveMap, SharedCoalescedMap},
};
use crate::entry::{DirtyRecord, FlushBatch, VisibleRef, WriteId};
use crate::sync::Arc;
use crate::sync::{BoundedQueue, Mutex, Ordering, SharedArc, SharedOptionArc, lock};
use ahash::{AHashMap, AHashSet, RandomState as AHashRandomState};
use std::borrow::Borrow;
use std::hash::{BuildHasher, Hash};
#[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
use std::time::Instant as StdInstant;

use scc::HashMap as ConcurrentHashMap;
use scc::hash_map::Entry as MapEntry;

#[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
use crate::perf::coalesced_perf_enabled;
#[cfg(all(not(feature = "loom"), feature = "dev-tools"))]
use crate::perf::*;

pub(super) const COALESCED_UNASSIGNED_WRITE_ID: WriteId = u64::MAX - 1;

pub(crate) struct CoalescedOverlay<K, V, H>
where
    K: Clone + Eq + Hash,
    H: BuildHasher + Clone,
{
    pub(super) active: SharedArc<CoalescedActiveMap<K, V, H>>,
    pub(super) active_publish: Mutex<()>,
    pub(super) draining: Mutex<Option<SharedCoalescedMap<K, V, H>>>,
    pub(super) draining_shared: SharedOptionArc<CoalescedActiveMap<K, V, H>>,
    pub(super) inflight: Mutex<Option<Arc<CoalescedInflight<K, V>>>>,
    pub(super) inflight_shared: SharedOptionArc<CoalescedInflight<K, V>>,
    pub(super) compact_scratch: BoundedQueue<CoalescedCompactScratch<K, V>>,
    pub(super) build_hasher: H,
    pub(super) map_capacity: usize,
}

impl<K, V, H> CoalescedOverlay<K, V, H>
where
    K: Clone + Eq + Hash,
    H: BuildHasher + Clone,
{
    pub(super) fn dirty_len(&self) -> usize {
        self.active.load_full().len()
            + lock(&self.inflight)
                .as_ref()
                .map_or(0, |current| current.batch.len())
            + lock(&self.draining)
                .as_ref()
                .map_or(0, |snapshot| snapshot.len())
    }

    pub(super) fn read_visible_ref<Q>(&self, key: &Q) -> Option<VisibleRef>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.read_active(key, |_, record| VisibleRef::Dirty(record.id))
            .or_else(|| self.read_inflight(key, |record| VisibleRef::Dirty(record.id)))
            .or_else(|| self.read_draining(key, |_, record| VisibleRef::Dirty(record.id)))
    }

    pub(super) fn read_active<Q, R>(
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

    pub(super) fn read_draining<Q, R>(
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

    pub(super) fn for_each_active_key(&self, mut reader: impl FnMut(&K))
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

    pub(super) fn for_each_draining_key(&self, mut reader: impl FnMut(&K))
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

    pub(super) fn visible_key_count(&self) -> usize
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

    pub(super) fn take_compact_scratch(&self, capacity: usize) -> CoalescedCompactScratch<K, V> {
        let mut scratch = self
            .compact_scratch
            .pop()
            .unwrap_or_else(|| CoalescedCompactScratch::with_capacity(capacity));
        scratch.ensure_capacity(capacity);
        scratch
    }

    pub(super) fn return_compact_scratch(&self, mut scratch: CoalescedCompactScratch<K, V>) {
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

    pub(super) fn publish_active_locked(&self, key: K, id: WriteId, value: V) -> bool {
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

    pub(super) fn update_or_insert<Q>(
        &self,
        lookup_key: &Q,
        owned_key: K,
        id: WriteId,
        value: V,
    ) -> bool
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

    pub(super) fn new_active_map(&self) -> SharedCoalescedMap<K, V, H> {
        Arc::new(ConcurrentHashMap::with_capacity_and_hasher(
            self.map_capacity,
            self.build_hasher.clone(),
        ))
    }

    pub(super) fn ensure_draining_snapshot_locked(&self) {
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

    pub(super) fn begin_snapshot_drain_batch(&self, limit: usize) -> Vec<DirtyRecord<K, V>> {
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

    pub(super) fn mark_flushed_if_current(&self, batch: &FlushBatch<K, V>) -> usize {
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

    pub(super) fn build_inflight_index(batch: &FlushBatch<K, V>) -> AHashMap<K, usize> {
        let mut index = AHashMap::with_capacity(batch.len());
        for (slot, record) in batch.iter().enumerate() {
            index.insert(record.key.clone(), slot);
        }
        index
    }

    pub(super) fn read_inflight<Q, R>(
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

    pub(super) fn with_inflight<R>(
        &self,
        reader: impl FnOnce(Option<&CoalescedInflight<K, V>>) -> R,
    ) -> R {
        self.inflight_shared
            .with(|inflight| reader(inflight.map(Arc::as_ref)))
    }

    pub(super) fn backlog_counts(&self) -> DirtyBacklogCounts {
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

impl<K, V, H> CacheLogMap<K, V, H>
where
    K: Clone + Eq + Hash,
    H: BuildHasher + Clone,
{
    pub(super) fn insert_dirty_batch_coalesced(&self, entries: Vec<(K, V)>) -> Vec<WriteId> {
        if entries.is_empty() {
            return Vec::new();
        }

        let mut seen = AHashSet::with_capacity(entries.len());
        let all_unique = entries.iter().all(|(key, _)| seen.insert(key));
        if all_unique {
            return self.publish_coalesced_batch(entries);
        }

        let mut scratch = self.take_coalesced_compact_scratch(entries.len());
        Self::compact_last_write_wins_into(entries, &mut scratch);
        let ids = self.publish_coalesced_batch_from_scratch(&mut scratch.compacted);
        self.return_coalesced_compact_scratch(scratch);
        ids
    }

    pub(super) fn insert_dirty_batch_coalesced_without_ids(&self, entries: Vec<(K, V)>) -> usize {
        if entries.is_empty() {
            return 0;
        }

        let mut seen = AHashSet::with_capacity(entries.len());
        let all_unique = entries.iter().all(|(key, _)| seen.insert(key));
        if all_unique {
            return self.publish_coalesced_batch_without_ids(entries);
        }

        let mut scratch = self.take_coalesced_compact_scratch(entries.len());
        Self::compact_last_write_wins_into(entries, &mut scratch);
        let written = self.publish_coalesced_batch_without_ids_from_scratch(&mut scratch.compacted);
        self.return_coalesced_compact_scratch(scratch);
        written
    }

    pub(super) fn publish_coalesced_active(&self, key: K, id: WriteId, value: V) -> bool {
        let _publish = lock(&self.coalesced.active_publish);
        self.remove_visible_clean_if_present(&key);
        self.coalesced.publish_active_locked(key, id, value)
    }

    pub(super) fn publish_coalesced_single_write(&self, key: K, value: V) -> WriteId {
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

    pub(super) fn publish_coalesced_single_write_without_id(&self, key: K, value: V) {
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

    fn publish_coalesced_batch_impl<I, F>(&self, entries: I, mut on_id: F) -> usize
    where
        I: IntoIterator<Item = (K, V)>,
        F: FnMut(WriteId),
    {
        let entries = entries.into_iter();
        let len = entries.size_hint().0;
        if len == 0 {
            return 0;
        }

        let id_base = self.reserve_write_ids(len);
        for (offset, (key, value)) in entries.enumerate() {
            let id = id_base.wrapping_add(offset as u64);
            let _ = self.publish_coalesced_active(key, id, value);
            on_id(id);
        }
        len
    }

    pub(super) fn publish_coalesced_batch(&self, entries: Vec<(K, V)>) -> Vec<WriteId> {
        let mut ids = Vec::with_capacity(entries.len());
        let _ = self.publish_coalesced_batch_impl(entries, |id| ids.push(id));
        ids
    }

    pub(super) fn publish_coalesced_batch_from_scratch(
        &self,
        entries: &mut Vec<(K, V)>,
    ) -> Vec<WriteId> {
        let mut ids = Vec::with_capacity(entries.len());
        let _ = self.publish_coalesced_batch_impl(entries.drain(..), |id| ids.push(id));
        ids
    }

    pub(super) fn publish_coalesced_batch_without_ids(&self, entries: Vec<(K, V)>) -> usize {
        self.publish_coalesced_batch_impl(entries, |_| ())
    }

    pub(super) fn publish_coalesced_batch_without_ids_from_scratch(
        &self,
        entries: &mut Vec<(K, V)>,
    ) -> usize {
        self.publish_coalesced_batch_impl(entries.drain(..), |_| ())
    }

    pub(super) fn take_coalesced_compact_scratch(
        &self,
        capacity: usize,
    ) -> CoalescedCompactScratch<K, V> {
        self.coalesced.take_compact_scratch(capacity)
    }

    pub(super) fn return_coalesced_compact_scratch(&self, scratch: CoalescedCompactScratch<K, V>) {
        self.coalesced.return_compact_scratch(scratch);
    }

    pub(super) fn flush_batch_coalesced(&self, limit: usize) -> FlushBatch<K, V> {
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

    pub(super) fn mark_flushed_coalesced(&self, batch: &FlushBatch<K, V>) -> usize {
        self.coalesced.mark_flushed_if_current(batch)
    }

    pub(super) fn read_coalesced_inflight<Q, R>(
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

    fn compact_last_write_wins_into(
        entries: Vec<(K, V)>,
        scratch: &mut CoalescedCompactScratch<K, V>,
    ) {
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
}

impl<V, H> CacheLogMap<Vec<u8>, V, H>
where
    H: BuildHasher + Clone,
{
    pub(super) fn borrowed_batch_has_duplicates(entries: &[(&[u8], &[u8])]) -> bool {
        if entries.len() < 2 {
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

    pub(super) fn publish_coalesced_borrowed_batch_without_ids<'a, F>(
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

    pub(super) fn compact_and_publish_coalesced_borrowed_batch_without_ids<'a, F>(
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

        super::borrowed_keys::with_borrowed_coalesced_keys(|scratch_keys| {
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

#[derive(Debug)]
pub(crate) struct CoalescedCompactScratch<K, V> {
    latest_slot: AHashMap<u64, DedupeSlots>,
    compacted: Vec<(K, V)>,
    fingerprint_hasher: AHashRandomState,
}

impl<K, V> CoalescedCompactScratch<K, V>
where
    K: Clone + Eq + Hash,
{
    pub(super) fn with_capacity(capacity: usize) -> Self {
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

    #[allow(dead_code)]
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
