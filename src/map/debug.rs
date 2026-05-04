use std::collections::BTreeSet;
use std::hash::{BuildHasher, Hash};

use crate::dirty_mode::DirtyMode;
use crate::sync::Ordering;

use super::{CacheLogMap, DirtyWriteMode, LowLevelMap, VisibleValue};

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
fn to_debug_record<K, V>(id: u64, key: K, value: V) -> DebugRecord<K, V> {
    DebugRecord { id, key, value }
}

#[cfg(any(test, feature = "loom"))]
fn collect_visible<K, V, H>(map: &CacheLogMap<K, V, H>) -> Vec<(K, DebugVisible<K, V>)>
where
    K: Clone + Eq + Hash + Ord,
    V: Clone,
    H: BuildHasher + Clone,
{
    let mut visible = Vec::new();
    if map.dirty_write_mode == DirtyWriteMode::CoalescedMap {
        collect_coalesced_visible(map, &mut visible);
    }
    collect_visible_entries(map, &mut visible);
    visible.sort_by(|left, right| left.0.cmp(&right.0));
    visible
}

#[cfg(any(test, feature = "loom"))]
fn collect_coalesced_visible<K, V, H>(
    map: &CacheLogMap<K, V, H>,
    visible: &mut Vec<(K, DebugVisible<K, V>)>,
) where
    K: Clone + Eq + Hash + Ord,
    V: Clone,
    H: BuildHasher + Clone,
{
    map.coalesced.for_each_active_key(|key| {
        let record = map
            .coalesced
            .read_active(key, |_, record| {
                to_debug_record(record.id, key.clone(), record.value.clone())
            })
            .expect("active coalesced key vanished during debug snapshot");
        visible.push((key.clone(), DebugVisible::Dirty(record)));
    });
    map.coalesced.with_inflight(|inflight| {
        if let Some(inflight) = inflight {
            for record in inflight.batch.iter() {
                if !visible
                    .iter()
                    .any(|(existing_key, _)| existing_key == &record.key)
                {
                    visible.push((
                        record.key.clone(),
                        DebugVisible::Dirty(to_debug_record(
                            record.id,
                            record.key.clone(),
                            record.value.clone(),
                        )),
                    ));
                }
            }
        }
    });
    map.coalesced.for_each_draining_key(|key| {
        if visible.iter().any(|(existing_key, _)| existing_key == key) {
            return;
        }
        let record = map
            .coalesced
            .read_draining(key, |_, record| {
                to_debug_record(record.id, key.clone(), record.value.clone())
            })
            .expect("draining coalesced key vanished during debug snapshot");
        visible.push((key.clone(), DebugVisible::Dirty(record)));
    });
}

#[cfg(any(test, feature = "loom"))]
fn collect_visible_entries<K, V, H>(
    map: &CacheLogMap<K, V, H>,
    visible: &mut Vec<(K, DebugVisible<K, V>)>,
) where
    K: Clone + Eq + Hash + Ord,
    V: Clone,
    H: BuildHasher + Clone,
{
    map.visible.iter_sync(|key, value| {
        let projected = match value {
            VisibleValue::Dirty(record) => DebugVisible::Dirty(to_debug_record(
                record.id,
                record.key.clone(),
                record.value.clone(),
            )),
            VisibleValue::Clean(record) => DebugVisible::Clean(to_debug_record(
                record.id,
                record.key.clone(),
                record.value.clone(),
            )),
        };
        visible.push((key.clone(), projected));
        true
    });
}

#[cfg(any(test, feature = "loom"))]
fn collect_strict_pending<K, V, H>(map: &CacheLogMap<K, V, H>) -> Vec<DebugRecord<K, V>>
where
    K: Clone + Eq + Hash + Ord,
    V: Clone,
    H: BuildHasher + Clone,
{
    map.dirty_mode
        .pending_records()
        .into_iter()
        .map(|record| to_debug_record(record.id, record.key.clone(), record.value.clone()))
        .collect()
}

#[cfg(any(test, feature = "loom"))]
fn collect_strict_inflight<K, V, H>(map: &CacheLogMap<K, V, H>) -> Vec<DebugRecord<K, V>>
where
    K: Clone + Eq + Hash + Ord,
    V: Clone,
    H: BuildHasher + Clone,
{
    map.dirty_mode
        .inflight_records()
        .into_iter()
        .map(|record| to_debug_record(record.id, record.key.clone(), record.value.clone()))
        .collect()
}

#[cfg(any(test, feature = "loom"))]
fn collect_coalesced_pending<K, V>(
    visible: &[(K, DebugVisible<K, V>)],
    inflight: &[DebugRecord<K, V>],
) -> Vec<DebugRecord<K, V>>
where
    K: Clone + Eq + Hash + Ord,
    V: Clone,
{
    let inflight_ids = inflight
        .iter()
        .map(|record| record.id)
        .collect::<BTreeSet<_>>();
    visible
        .iter()
        .filter_map(|(_, projected)| match projected {
            DebugVisible::Dirty(record) if !inflight_ids.contains(&record.id) => {
                Some(record.clone())
            }
            _ => None,
        })
        .collect()
}

#[cfg(any(test, feature = "loom"))]
fn collect_counters<K, V, H>(map: &CacheLogMap<K, V, H>) -> (u64, u64, usize)
where
    K: Clone + Eq + Hash + Ord,
    V: Clone,
    H: BuildHasher + Clone,
{
    (
        match map.dirty_write_mode {
            DirtyWriteMode::StrictLog => map.dirty_mode.next_write(),
            DirtyWriteMode::CoalescedMap => map.next_dirty_write.load(Ordering::Relaxed),
        },
        map.next_cache.load(Ordering::Relaxed),
        map.clean_count.load(Ordering::Relaxed),
    )
}

#[cfg(any(test, feature = "loom"))]
impl<K, V, H> CacheLogMap<K, V, H>
where
    K: Clone + Eq + Hash + Ord,
    V: Clone,
    H: BuildHasher + Clone,
{
    #[doc(hidden)]
    pub(crate) fn debug_snapshot(&self) -> DebugSnapshot<K, V> {
        let visible = collect_visible(self);
        let (dirty_pending, dirty_inflight) = match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => {
                (collect_strict_pending(self), collect_strict_inflight(self))
            }
            DirtyWriteMode::CoalescedMap => {
                let inflight = self.coalesced.with_inflight(|inflight| {
                    inflight
                        .map(|inflight| {
                            inflight
                                .batch
                                .iter()
                                .map(|record| {
                                    to_debug_record(
                                        record.id,
                                        record.key.clone(),
                                        record.value.clone(),
                                    )
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                });
                (collect_coalesced_pending(&visible, &inflight), inflight)
            }
        };
        let (next_write, next_cache, clean_count) = collect_counters(self);
        DebugSnapshot {
            visible,
            dirty_pending,
            dirty_inflight,
            next_write,
            next_cache,
            clean_count,
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
