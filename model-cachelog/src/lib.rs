use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

pub use cachelog_core::{
    CacheId, CleanRecord, DurableValue, InvariantReport, KeyId, ModelConfig, ModelError,
    ModelState, ModelStep, ValueId, VisibleRef, WriteId,
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ComparableDirtyRecord {
    pub id: WriteId,
    pub key: KeyId,
    pub value: ValueId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ComparableCleanRecord {
    pub id: CacheId,
    pub key: KeyId,
    pub value: ValueId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ComparableDurableValue {
    pub key: KeyId,
    pub value: ValueId,
    pub seq: WriteId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id")]
pub enum ComparableVisibleRef {
    Dirty(WriteId),
    Clean(CacheId),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
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

impl From<&ModelState> for ComparableState {
    fn from(state: &ModelState) -> Self {
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
            .filter_map(|record| {
                (record.present && record.id.is_some()).then(|| {
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
            })
            .collect();

        let write_hist = state
            .write_hist
            .iter()
            .filter_map(|record| {
                (record.present && record.id.is_some()).then(|| {
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
            })
            .collect();

        let cache_store = state
            .cache_store
            .iter()
            .filter_map(|record| {
                (record.present && record.id.is_some()).then(|| {
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
            })
            .collect();

        let durable = state
            .durable
            .iter()
            .enumerate()
            .filter_map(|(key, value)| {
                (value.present && value.seq.is_some()).then(|| {
                    (
                        key,
                        ComparableDurableValue {
                            key,
                            value: value.value,
                            seq: value.seq.expect("present durable has seq"),
                        },
                    )
                })
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

#[cfg(feature = "tla-connect")]
pub mod tla;
