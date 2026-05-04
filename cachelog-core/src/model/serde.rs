//! Serde-serializable / comparable types derived from the core model.
//! These strip absent fields and use ordered maps for deterministic comparison.

#[cfg(not(creusot))]
use std::collections::{BTreeMap, BTreeSet};

#[cfg(all(feature = "serde", not(creusot)))]
use ::serde::{Deserialize, Serialize};

#[cfg(creusot)]
use creusot_std::model::DeepModel;

#[cfg(not(creusot))]
use thiserror::Error;

use super::state::{CacheId, KeyId, ModelState, ValueId, VisibleRef, WriteId};

#[cfg_attr(all(feature = "serde", not(creusot)), derive(Serialize, Deserialize))]
#[cfg_attr(creusot, derive(DeepModel))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComparableDirtyRecord {
    pub id: WriteId,
    pub key: KeyId,
    pub value: ValueId,
}

#[cfg_attr(all(feature = "serde", not(creusot)), derive(Serialize, Deserialize))]
#[cfg_attr(creusot, derive(DeepModel))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComparableCleanRecord {
    pub id: CacheId,
    pub key: KeyId,
    pub value: ValueId,
}

#[cfg_attr(all(feature = "serde", not(creusot)), derive(Serialize, Deserialize))]
#[cfg_attr(creusot, derive(DeepModel))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComparableDurableValue {
    pub key: KeyId,
    pub value: ValueId,
    pub seq: WriteId,
}

#[cfg_attr(all(feature = "serde", not(creusot)), derive(Serialize, Deserialize))]
#[cfg_attr(creusot, derive(DeepModel))]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
#[cfg_attr(
    all(feature = "serde", not(creusot)),
    serde(tag = "kind", content = "id")
)]
pub enum ComparableVisibleRef {
    Dirty(WriteId),
    Clean(CacheId),
}

#[cfg_attr(all(feature = "serde", not(creusot)), derive(Serialize, Deserialize))]
#[cfg_attr(creusot, derive(DeepModel))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComparableState {
    pub visible: BTreeMap<KeyId, ComparableVisibleRef>,
    pub write_store: BTreeMap<WriteId, ComparableDirtyRecord>,
    pub write_hist: BTreeMap<WriteId, ComparableDirtyRecord>,
    pub dirty_q: Vec<WriteId>,
    pub cache_store: BTreeMap<CacheId, ComparableCleanRecord>,
    pub durable: BTreeMap<KeyId, ComparableDurableValue>,
    pub flushed: Vec<WriteId>,
    pub created_dirty: BTreeSet<WriteId>,
    pub next_write: usize,
    pub next_cache: usize,
    pub crashed: bool,
    pub bad_read: bool,
}

impl ComparableState {
    pub fn from_state(state: &ModelState) -> Self {
        let visible = state
            .visible
            .iter()
            .enumerate()
            .filter_map(|(key, entry)| match *entry {
                VisibleRef::None => None,
                VisibleRef::Dirty(id) => Some((key, ComparableVisibleRef::Dirty(id))),
                VisibleRef::Clean(id) => Some((key, ComparableVisibleRef::Clean(id))),
            })
            .collect();

        let write_store = state
            .write_store
            .iter()
            .filter(|record| record.present && record.id.is_some())
            .map(|record| {
                let id = record.id.expect("present dirty record has id");
                (
                    id,
                    ComparableDirtyRecord {
                        id,
                        key: record.key,
                        value: record.value,
                    },
                )
            })
            .collect();

        let write_hist = state
            .write_hist
            .iter()
            .filter(|record| record.present && record.id.is_some())
            .map(|record| {
                let id = record.id.expect("present hist record has id");
                (
                    id,
                    ComparableDirtyRecord {
                        id,
                        key: record.key,
                        value: record.value,
                    },
                )
            })
            .collect();

        let cache_store = state
            .cache_store
            .iter()
            .filter(|record| record.present && record.id.is_some())
            .map(|record| {
                let id = record.id.expect("present clean record has id");
                (
                    id,
                    ComparableCleanRecord {
                        id,
                        key: record.key,
                        value: record.value,
                    },
                )
            })
            .collect();

        let durable = state
            .durable
            .iter()
            .enumerate()
            .filter(|(_, value)| value.present && value.seq.is_some())
            .map(|(key, value)| {
                (
                    key,
                    ComparableDurableValue {
                        key,
                        value: value.value,
                        seq: value.seq.expect("present durable has seq"),
                    },
                )
            })
            .collect();

        let created_dirty = state
            .created_dirty
            .iter()
            .enumerate()
            .filter_map(|(id, present)| (*present).then_some(id))
            .collect();

        Self {
            visible,
            write_store,
            write_hist,
            dirty_q: state.dirty_q.clone(),
            cache_store,
            durable,
            flushed: state.flushed.clone(),
            created_dirty,
            next_write: state.next_write,
            next_cache: state.next_cache,
            crashed: state.crashed,
            bad_read: state.bad_read,
        }
    }
}

impl ModelState {
    #[cfg(not(creusot))]
    pub fn comparable(&self) -> ComparableState {
        ComparableState::from_state(self)
    }
}

// ---------------------------------------------------------------------------

#[cfg_attr(all(feature = "serde", not(creusot)), derive(Serialize, Deserialize))]
#[cfg_attr(creusot, derive(DeepModel))]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum InvariantMode {
    #[default]
    StrictLog,
    CoalescedMap,
}

#[cfg_attr(all(feature = "serde", not(creusot)), derive(Serialize))]
#[cfg_attr(creusot, derive(DeepModel))]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InvariantReport {
    pub failures: Vec<&'static str>,
}

impl InvariantReport {
    #[cfg_attr(feature = "creusot", ensures(result == (self.failures@.len() == 0)))]
    pub fn is_ok(&self) -> bool {
        self.failures.is_empty()
    }
}

#[cfg_attr(not(creusot), derive(Error))]
#[cfg_attr(creusot, derive(DeepModel))]
#[derive(Debug, Eq, PartialEq)]
pub enum ModelError {
    #[cfg_attr(not(creusot), error("key {0} is out of range for config"))]
    InvalidKey(KeyId),
    #[cfg_attr(not(creusot), error("value {0} is out of range for config"))]
    InvalidValue(ValueId),
    #[cfg_attr(not(creusot), error("write id {0} is out of range for config"))]
    InvalidWriteId(WriteId),
    #[cfg_attr(not(creusot), error("cache id {0} is out of range for config"))]
    InvalidCacheId(CacheId),
}

#[cfg_attr(all(feature = "serde", not(creusot)), derive(Serialize, Deserialize))]
#[cfg_attr(creusot, derive(DeepModel))]
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(
    all(feature = "serde", not(creusot)),
    serde(tag = "action", rename_all = "camelCase")
)]
pub enum ModelStep {
    WriterWrite { key: KeyId, value: ValueId },
    FlusherFlushNext,
    FlusherDrop { id: WriteId },
    CacheInsert { key: KeyId },
    CacheDropRecord { id: CacheId },
    CacheCleanupVisible { id: CacheId },
    ReaderStep { key: KeyId },
    Crash,
}
