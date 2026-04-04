use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub type KeyId = usize;
pub type ValueId = usize;
pub type WriteId = usize;
pub type CacheId = usize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id")]
pub enum VisibleRef {
    None,
    Dirty(WriteId),
    Clean(CacheId),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DirtyRecord {
    pub present: bool,
    pub id: Option<WriteId>,
    pub key: KeyId,
    pub value: ValueId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CleanRecord {
    pub present: bool,
    pub id: Option<CacheId>,
    pub key: KeyId,
    pub value: ValueId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DurableValue {
    pub present: bool,
    pub value: ValueId,
    pub seq: Option<WriteId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
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

    fn valid_key(self, key: KeyId) -> bool {
        key < self.key_count
    }

    fn valid_value(self, value: ValueId) -> bool {
        value < self.value_count
    }

    fn valid_write(self, id: WriteId) -> bool {
        id < self.max_write
    }

    fn valid_cache(self, id: CacheId) -> bool {
        id < self.max_cache
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Ord, PartialOrd)]
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase")]
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

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct InvariantReport {
    pub failures: Vec<&'static str>,
}

impl InvariantReport {
    pub fn is_ok(&self) -> bool {
        self.failures.is_empty()
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum ModelError {
    #[error("key {0} is out of range for config")]
    InvalidKey(KeyId),
    #[error("value {0} is out of range for config")]
    InvalidValue(ValueId),
    #[error("write id {0} is out of range for config")]
    InvalidWriteId(WriteId),
    #[error("cache id {0} is out of range for config")]
    InvalidCacheId(CacheId),
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

    pub fn apply_step(&mut self, config: ModelConfig, step: ModelStep) -> Result<(), ModelError> {
        match step {
            ModelStep::WriterWrite { key, value } => self.writer_write(config, key, value),
            ModelStep::FlusherFlushNext => self.flusher_flush_next(config),
            ModelStep::FlusherDrop { id } => self.flusher_drop(config, id),
            ModelStep::CacheInsert { key } => self.cache_insert(config, key),
            ModelStep::CacheDropRecord { id } => self.cache_drop_record(config, id),
            ModelStep::CacheCleanupVisible { id } => self.cache_cleanup_visible(config, id),
            ModelStep::ReaderStep { key } => self.reader_step(config, key),
            ModelStep::Crash => self.crash(config),
        }
    }

    pub fn writer_write(
        &mut self,
        config: ModelConfig,
        key: KeyId,
        value: ValueId,
    ) -> Result<(), ModelError> {
        self.check_key_value(config, key, value)?;
        if self.crashed || self.next_write >= config.max_write {
            return Ok(());
        }

        let id = self.next_write;
        let record = DirtyRecord::present(id, key, value);
        self.write_store[id] = record.clone();
        self.write_hist[id] = record;
        self.dirty_q.push(id);
        self.visible[key] = VisibleRef::Dirty(id);
        self.created_dirty[id] = true;
        self.next_write += 1;
        Ok(())
    }

    pub fn flusher_flush_next(&mut self, config: ModelConfig) -> Result<(), ModelError> {
        if self.crashed {
            return Ok(());
        }
        if let Some(id) = self.dirty_q.first().copied() {
            self.check_write(config, id)?;
            let record = &self.write_hist[id];
            if record.present {
                self.durable[record.key] = DurableValue::present(record.value, id);
                self.flushed.push(id);
                self.dirty_q.remove(0);
            }
        }
        Ok(())
    }

    pub fn flusher_drop(&mut self, config: ModelConfig, id: WriteId) -> Result<(), ModelError> {
        self.check_write(config, id)?;
        if self.crashed {
            return Ok(());
        }
        if self.flushed.contains(&id) && self.write_store[id].present {
            self.write_store[id] = DirtyRecord::absent();
            for visible in &mut self.visible {
                if *visible == VisibleRef::Dirty(id) {
                    *visible = VisibleRef::None;
                }
            }
        }
        Ok(())
    }

    pub fn cache_insert(&mut self, config: ModelConfig, key: KeyId) -> Result<(), ModelError> {
        self.check_key(config, key)?;
        if self.crashed || self.next_cache >= config.max_cache {
            return Ok(());
        }
        if self.durable[key].present && self.visible[key] == VisibleRef::None {
            let id = self.next_cache;
            self.cache_store[id] = CleanRecord::present(id, key, self.durable[key].value);
            self.visible[key] = VisibleRef::Clean(id);
            self.next_cache += 1;
        }
        Ok(())
    }

    pub fn cache_drop_record(
        &mut self,
        config: ModelConfig,
        id: CacheId,
    ) -> Result<(), ModelError> {
        self.check_cache(config, id)?;
        if self.crashed {
            return Ok(());
        }
        if self.cache_store[id].present {
            self.cache_store[id] = CleanRecord::absent();
        }
        Ok(())
    }

    pub fn cache_cleanup_visible(
        &mut self,
        config: ModelConfig,
        id: CacheId,
    ) -> Result<(), ModelError> {
        self.check_cache(config, id)?;
        if self.crashed {
            return Ok(());
        }
        if !self.cache_store[id].present {
            for visible in &mut self.visible {
                if *visible == VisibleRef::Clean(id) {
                    *visible = VisibleRef::None;
                }
            }
        }
        Ok(())
    }

    pub fn reader_step(&mut self, config: ModelConfig, key: KeyId) -> Result<(), ModelError> {
        self.check_key(config, key)?;
        if self.crashed {
            return Ok(());
        }
        match self.visible[key] {
            VisibleRef::None => {}
            VisibleRef::Dirty(id) => {
                self.check_write(config, id)?;
                if !self.write_store[id].present || self.write_store[id].key != key {
                    self.bad_read = true;
                }
            }
            VisibleRef::Clean(id) => {
                self.check_cache(config, id)?;
                if self.cache_store[id].present && self.cache_store[id].key != key {
                    self.bad_read = true;
                }
            }
        }
        Ok(())
    }

    pub fn crash(&mut self, config: ModelConfig) -> Result<(), ModelError> {
        if self.crashed {
            return Ok(());
        }
        self.visible = vec![VisibleRef::None; config.key_count];
        self.write_store = (0..config.max_write)
            .map(|_| DirtyRecord::absent())
            .collect();
        self.dirty_q.clear();
        self.cache_store = (0..config.max_cache)
            .map(|_| CleanRecord::absent())
            .collect();
        self.crashed = true;
        Ok(())
    }

    pub fn comparable(&self) -> ComparableState {
        ComparableState::from(self)
    }

    pub fn check_invariants(&self, config: ModelConfig) -> InvariantReport {
        let mut report = InvariantReport::default();

        if !self.type_ok(config) {
            report.failures.push("TypeOK");
        }
        if !self.dirty_refs_live(config) {
            report.failures.push("DirtyRefsLive");
        }
        if !strictly_increasing(&self.dirty_q) {
            report.failures.push("DirtyQOrdered");
        }
        if !strictly_increasing(&self.flushed) {
            report.failures.push("FlushedOrdered");
        }
        if !self.durable_matches_flushed(config) {
            report.failures.push("DurableMatchesFlushed");
        }
        if !self.no_lost_dirty() {
            report.failures.push("NoLostDirty");
        }
        if self.bad_read {
            report.failures.push("NoBadRead");
        }

        report
    }

    pub fn type_ok(&self, config: ModelConfig) -> bool {
        self.visible.len() == config.key_count
            && self.write_store.len() == config.max_write
            && self.write_hist.len() == config.max_write
            && self.cache_store.len() == config.max_cache
            && self.durable.len() == config.key_count
            && self.created_dirty.len() == config.max_write
            && self.next_write <= config.max_write
            && self.next_cache <= config.max_cache
            && self.visible.iter().all(|entry| match *entry {
                VisibleRef::None => true,
                VisibleRef::Dirty(id) => config.valid_write(id),
                VisibleRef::Clean(id) => config.valid_cache(id),
            })
            && self
                .write_store
                .iter()
                .all(|record| self.valid_dirty_record(config, record))
            && self
                .write_hist
                .iter()
                .all(|record| self.valid_dirty_record(config, record))
            && self
                .cache_store
                .iter()
                .all(|record| self.valid_clean_record(config, record))
            && self
                .durable
                .iter()
                .all(|value| self.valid_durable_value(config, value))
            && self.dirty_q.iter().all(|&id| config.valid_write(id))
            && self.flushed.iter().all(|&id| config.valid_write(id))
    }

    pub fn dirty_refs_live(&self, config: ModelConfig) -> bool {
        self.visible.iter().all(|visible| match *visible {
            VisibleRef::None | VisibleRef::Clean(_) => true,
            VisibleRef::Dirty(id) => {
                config.valid_write(id)
                    && self.write_store[id].present
                    && self.write_store[id].id == Some(id)
            }
        })
    }

    pub fn durable_matches_flushed(&self, config: ModelConfig) -> bool {
        self.durable == apply_flushed(config, &self.flushed, &self.write_hist)
    }

    pub fn no_lost_dirty(&self) -> bool {
        if self.crashed {
            return true;
        }
        self.created_dirty.iter().enumerate().all(|(id, created)| {
            !*created
                || self.dirty_q.contains(&id)
                || self.flushed.contains(&id)
                || self.visible.contains(&VisibleRef::Dirty(id))
                || self.write_store[id].present
        })
    }

    fn check_key(&self, config: ModelConfig, key: KeyId) -> Result<(), ModelError> {
        if config.valid_key(key) {
            Ok(())
        } else {
            Err(ModelError::InvalidKey(key))
        }
    }

    fn check_key_value(
        &self,
        config: ModelConfig,
        key: KeyId,
        value: ValueId,
    ) -> Result<(), ModelError> {
        self.check_key(config, key)?;
        if config.valid_value(value) {
            Ok(())
        } else {
            Err(ModelError::InvalidValue(value))
        }
    }

    fn check_write(&self, config: ModelConfig, id: WriteId) -> Result<(), ModelError> {
        if config.valid_write(id) {
            Ok(())
        } else {
            Err(ModelError::InvalidWriteId(id))
        }
    }

    fn check_cache(&self, config: ModelConfig, id: CacheId) -> Result<(), ModelError> {
        if config.valid_cache(id) {
            Ok(())
        } else {
            Err(ModelError::InvalidCacheId(id))
        }
    }

    fn valid_dirty_record(&self, config: ModelConfig, record: &DirtyRecord) -> bool {
        if !record.present {
            return true;
        }
        match record.id {
            Some(id) => {
                config.valid_write(id)
                    && config.valid_key(record.key)
                    && config.valid_value(record.value)
            }
            None => false,
        }
    }

    fn valid_clean_record(&self, config: ModelConfig, record: &CleanRecord) -> bool {
        if !record.present {
            return true;
        }
        match record.id {
            Some(id) => {
                config.valid_cache(id)
                    && config.valid_key(record.key)
                    && config.valid_value(record.value)
            }
            None => false,
        }
    }

    fn valid_durable_value(&self, config: ModelConfig, value: &DurableValue) -> bool {
        if !value.present {
            return true;
        }
        match value.seq {
            Some(seq) => config.valid_write(seq) && config.valid_value(value.value),
            None => false,
        }
    }
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

fn strictly_increasing(items: &[usize]) -> bool {
    items.windows(2).all(|pair| pair[0] < pair[1])
}

fn apply_flushed(
    config: ModelConfig,
    flushed: &[WriteId],
    write_hist: &[DirtyRecord],
) -> Vec<DurableValue> {
    let mut durable = vec![DurableValue::absent(); config.key_count];
    for &id in flushed {
        if let Some(record) = write_hist.get(id).filter(|record| record.present) {
            durable[record.key] = DurableValue::present(record.value, id);
        }
    }
    durable
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_invariants(state: &ModelState, cfg: ModelConfig) {
        let report = state.check_invariants(cfg);
        assert!(report.is_ok(), "{report:?}");
    }

    #[test]
    fn write_flush_drop_preserves_invariants() {
        let cfg = ModelConfig::tla_small();
        let mut state = ModelState::new(cfg);

        state.writer_write(cfg, 0, 1).unwrap();
        state.flusher_flush_next(cfg).unwrap();
        state.flusher_drop(cfg, 0).unwrap();

        assert_invariants(&state, cfg);
        assert_eq!(state.durable[0], DurableValue::present(1, 0));
        assert_eq!(state.visible[0], VisibleRef::None);
    }

    #[test]
    fn stale_clean_ref_window_is_allowed() {
        let cfg = ModelConfig::tla_small();
        let mut state = ModelState::new(cfg);

        state.writer_write(cfg, 0, 1).unwrap();
        state.flusher_flush_next(cfg).unwrap();
        state.flusher_drop(cfg, 0).unwrap();
        state.cache_insert(cfg, 0).unwrap();
        let cache_id = match state.visible[0] {
            VisibleRef::Clean(id) => id,
            other => panic!("expected clean ref, got {other:?}"),
        };
        state.cache_drop_record(cfg, cache_id).unwrap();
        state.reader_step(cfg, 0).unwrap();

        assert!(!state.bad_read);
        assert_invariants(&state, cfg);
    }

    #[test]
    fn cleanup_removes_stale_clean_ref() {
        let cfg = ModelConfig::tla_small();
        let mut state = ModelState::new(cfg);

        state.writer_write(cfg, 0, 1).unwrap();
        state.flusher_flush_next(cfg).unwrap();
        state.flusher_drop(cfg, 0).unwrap();
        state.cache_insert(cfg, 0).unwrap();
        let cache_id = match state.visible[0] {
            VisibleRef::Clean(id) => id,
            _ => unreachable!(),
        };
        state.cache_drop_record(cfg, cache_id).unwrap();
        state.cache_cleanup_visible(cfg, cache_id).unwrap();

        assert_eq!(state.visible[0], VisibleRef::None);
        assert_invariants(&state, cfg);
    }

    #[test]
    fn crash_preserves_durable_prefix() {
        let cfg = ModelConfig::tla_small();
        let mut state = ModelState::new(cfg);

        state.writer_write(cfg, 1, 1).unwrap();
        state.flusher_flush_next(cfg).unwrap();
        let durable_before = state.durable.clone();
        state.crash(cfg).unwrap();

        assert_eq!(state.durable, durable_before);
        assert!(state.crashed);
        assert_invariants(&state, cfg);
    }

    #[test]
    fn writer_write_preserves_invariants_from_init() {
        let cfg = ModelConfig::tla_small();
        let mut state = ModelState::new(cfg);

        state.writer_write(cfg, 1, 0).unwrap();

        assert_eq!(state.visible[1], VisibleRef::Dirty(0));
        assert_eq!(state.dirty_q, vec![0]);
        assert_invariants(&state, cfg);
    }

    #[test]
    fn flusher_flush_next_preserves_invariants_on_empty_and_nonempty_queue() {
        let cfg = ModelConfig::tla_small();
        let mut state = ModelState::new(cfg);

        state.flusher_flush_next(cfg).unwrap();
        assert_invariants(&state, cfg);

        state.writer_write(cfg, 0, 1).unwrap();
        state.flusher_flush_next(cfg).unwrap();

        assert_eq!(state.flushed, vec![0]);
        assert_eq!(state.dirty_q, Vec::<usize>::new());
        assert_eq!(state.durable[0], DurableValue::present(1, 0));
        assert_invariants(&state, cfg);
    }

    #[test]
    fn flusher_drop_clears_matching_visible_dirty_ref() {
        let cfg = ModelConfig::tla_small();
        let mut state = ModelState::new(cfg);

        state.writer_write(cfg, 0, 1).unwrap();
        state.flusher_flush_next(cfg).unwrap();
        state.flusher_drop(cfg, 0).unwrap();

        assert_eq!(state.visible[0], VisibleRef::None);
        assert!(!state.write_store[0].present);
        assert_invariants(&state, cfg);
    }

    #[test]
    fn cache_insert_preserves_invariants_when_durable_exists() {
        let cfg = ModelConfig::tla_small();
        let mut state = ModelState::new(cfg);

        state.writer_write(cfg, 1, 1).unwrap();
        state.flusher_flush_next(cfg).unwrap();
        state.flusher_drop(cfg, 0).unwrap();
        state.cache_insert(cfg, 1).unwrap();

        assert_eq!(state.visible[1], VisibleRef::Clean(0));
        assert!(state.cache_store[0].present);
        assert_invariants(&state, cfg);
    }

    #[test]
    fn cache_drop_record_preserves_invariants_with_stale_visible_clean_ref() {
        let cfg = ModelConfig::tla_small();
        let mut state = ModelState::new(cfg);

        state.writer_write(cfg, 1, 1).unwrap();
        state.flusher_flush_next(cfg).unwrap();
        state.flusher_drop(cfg, 0).unwrap();
        state.cache_insert(cfg, 1).unwrap();
        state.cache_drop_record(cfg, 0).unwrap();

        assert_eq!(state.visible[1], VisibleRef::Clean(0));
        assert!(!state.cache_store[0].present);
        assert_invariants(&state, cfg);
    }

    #[test]
    fn reader_step_preserves_invariants_on_valid_states() {
        let cfg = ModelConfig::tla_small();
        let mut state = ModelState::new(cfg);

        state.writer_write(cfg, 0, 1).unwrap();
        state.reader_step(cfg, 0).unwrap();
        assert!(!state.bad_read);
        assert_invariants(&state, cfg);

        state.flusher_flush_next(cfg).unwrap();
        state.flusher_drop(cfg, 0).unwrap();
        state.cache_insert(cfg, 0).unwrap();
        state.reader_step(cfg, 0).unwrap();
        assert!(!state.bad_read);
        assert_invariants(&state, cfg);
    }

    #[test]
    fn crash_from_clean_visible_state_preserves_invariants() {
        let cfg = ModelConfig::tla_small();
        let mut state = ModelState::new(cfg);

        state.writer_write(cfg, 0, 1).unwrap();
        state.flusher_flush_next(cfg).unwrap();
        state.flusher_drop(cfg, 0).unwrap();
        state.cache_insert(cfg, 0).unwrap();
        state.crash(cfg).unwrap();

        assert_eq!(state.visible, vec![VisibleRef::None, VisibleRef::None]);
        assert!(state.crashed);
        assert_invariants(&state, cfg);
    }
}
