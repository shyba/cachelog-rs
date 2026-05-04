//! Core state types for the cachelog model.

#[cfg(all(feature = "serde", not(creusot)))]
use serde::{Deserialize, Serialize};

#[cfg(feature = "creusot")]
use creusot_std::model::DeepModel;

pub type KeyId = usize;
pub type ValueId = usize;
pub type WriteId = usize;
pub type CacheId = usize;

// `cachelog-core` models the strict id-bearing surface. Coalesced no-id
// batches are common latest-value semantics, not a second strict-log model.
// See `model-cachelog/formal/PersistBatchBoundary.md` for the current mapping.

#[cfg_attr(all(feature = "serde", not(creusot)), derive(Serialize, Deserialize))]
#[cfg_attr(creusot, derive(DeepModel))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(
    all(feature = "serde", not(creusot)),
    serde(tag = "kind", content = "id")
)]
pub enum VisibleRef {
    None,
    Dirty(WriteId),
    Clean(CacheId),
}

#[cfg_attr(all(feature = "serde", not(creusot)), derive(Serialize, Deserialize))]
#[cfg_attr(creusot, derive(DeepModel))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirtyRecord {
    pub present: bool,
    pub id: Option<WriteId>,
    pub key: KeyId,
    pub value: ValueId,
}

#[cfg_attr(all(feature = "serde", not(creusot)), derive(Serialize, Deserialize))]
#[cfg_attr(creusot, derive(DeepModel))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CleanRecord {
    pub present: bool,
    pub id: Option<CacheId>,
    pub key: KeyId,
    pub value: ValueId,
}

#[cfg_attr(all(feature = "serde", not(creusot)), derive(Serialize, Deserialize))]
#[cfg_attr(creusot, derive(DeepModel))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurableValue {
    pub present: bool,
    pub value: ValueId,
    pub seq: Option<WriteId>,
}

#[cfg_attr(all(feature = "serde", not(creusot)), derive(Serialize, Deserialize))]
#[cfg_attr(creusot, derive(DeepModel))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelConfig {
    pub key_count: usize,
    pub value_count: usize,
    pub max_write: usize,
    pub max_cache: usize,
}

impl ModelConfig {
    pub const fn new(
        key_count: usize,
        value_count: usize,
        max_write: usize,
        max_cache: usize,
    ) -> Self {
        Self {
            key_count,
            value_count,
            max_write,
            max_cache,
        }
    }

    pub const fn tla_small() -> Self {
        Self::new(2, 2, 3, 2)
    }

    pub(crate) fn valid_key(self, key: KeyId) -> bool {
        key < self.key_count
    }

    pub(crate) fn valid_value(self, value: ValueId) -> bool {
        value < self.value_count
    }

    pub(crate) fn valid_write(self, id: WriteId) -> bool {
        id < self.max_write
    }

    pub(crate) fn valid_cache(self, id: CacheId) -> bool {
        id < self.max_cache
    }
}

#[cfg_attr(all(feature = "serde", not(creusot)), derive(Serialize, Deserialize))]
#[cfg_attr(creusot, derive(DeepModel))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelState {
    pub visible: Vec<VisibleRef>,
    pub write_store: Vec<DirtyRecord>,
    pub write_hist: Vec<DirtyRecord>,
    pub dirty_q: Vec<WriteId>,
    pub cache_store: Vec<CleanRecord>,
    pub durable: Vec<DurableValue>,
    pub flushed: Vec<WriteId>,
    pub created_dirty: Vec<bool>,
    pub next_write: usize,
    pub next_cache: usize,
    pub crashed: bool,
    pub bad_read: bool,
}

impl DirtyRecord {
    pub fn absent() -> Self {
        Self {
            present: false,
            id: None,
            key: 0,
            value: 0,
        }
    }

    pub fn present(id: WriteId, key: KeyId, value: ValueId) -> Self {
        Self {
            present: true,
            id: Some(id),
            key,
            value,
        }
    }
}

impl CleanRecord {
    pub fn absent() -> Self {
        Self {
            present: false,
            id: None,
            key: 0,
            value: 0,
        }
    }

    pub fn present(id: CacheId, key: KeyId, value: ValueId) -> Self {
        Self {
            present: true,
            id: Some(id),
            key,
            value,
        }
    }
}

impl DurableValue {
    pub fn absent() -> Self {
        Self {
            present: false,
            value: 0,
            seq: None,
        }
    }

    pub fn present(value: ValueId, seq: WriteId) -> Self {
        Self {
            present: true,
            value,
            seq: Some(seq),
        }
    }
}

impl ModelState {
    pub fn new(config: ModelConfig) -> Self {
        Self {
            visible: vec![VisibleRef::None; config.key_count],
            write_store: (0..config.max_write)
                .map(|_| DirtyRecord::absent())
                .collect(),
            write_hist: (0..config.max_write)
                .map(|_| DirtyRecord::absent())
                .collect(),
            dirty_q: Vec::new(),
            cache_store: (0..config.max_cache)
                .map(|_| CleanRecord::absent())
                .collect(),
            durable: (0..config.key_count)
                .map(|_| DurableValue::absent())
                .collect(),
            flushed: Vec::new(),
            created_dirty: vec![false; config.max_write],
            next_write: 0,
            next_cache: 0,
            crashed: false,
            bad_read: false,
        }
    }
}
