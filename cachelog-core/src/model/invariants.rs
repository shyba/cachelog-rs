//! Invariant checking for model state.

#[cfg(feature = "creusot")]
use creusot_std::macros::{ensures, requires};

use super::serde::{InvariantMode, InvariantReport, ModelError};
use super::state::{
    CacheId, CleanRecord, DirtyRecord, DurableValue, KeyId, ModelConfig, ModelState, ValueId,
    VisibleRef, WriteId,
};
use super::visibility::contains_visible_dirty;

impl ModelState {
    #[cfg(feature = "creusot")]
    #[cfg_attr(feature = "creusot", requires(crate::proofs::valid_config(config)))]
    #[cfg_attr(
        feature = "creusot",
        ensures(result@.is_ok() == crate::proofs::inv_for_mode(*self, config, mode))
    )]
    pub fn check_invariants_for_mode(
        &self,
        config: ModelConfig,
        mode: InvariantMode,
    ) -> InvariantReport {
        let ok = self.type_ok(config)
            && self.dirty_refs_live(config)
            && self.clean_refs_key_consistent(config)
            && self.durable_matches_flushed(config)
            && self.no_lost_dirty(config)
            && !self.bad_read
            && match mode {
                InvariantMode::StrictLog => {
                    self.dirty_q_ordered()
                        && self.flushed_ordered()
                        && self.queued_after_flushed()
                        && self.all_ids_lt_next_write()
                }
                InvariantMode::CoalescedMap => self.all_ids_lt_next_write(),
            };

        if ok {
            InvariantReport::default()
        } else {
            InvariantReport {
                failures: vec!["InvariantFailure"],
            }
        }
    }

    #[cfg(feature = "creusot")]
    pub fn check_invariants(&self, config: ModelConfig) -> InvariantReport {
        self.check_invariants_for_mode(config, InvariantMode::StrictLog)
    }

    #[cfg(not(feature = "creusot"))]
    pub fn check_invariants_for_mode(
        &self,
        config: ModelConfig,
        mode: InvariantMode,
    ) -> InvariantReport {
        let mut report = InvariantReport::default();

        if !self.type_ok(config) {
            report.failures.push("TypeOK");
        }
        if !self.dirty_refs_live(config) {
            report.failures.push("DirtyRefsLive");
        }
        if !self.clean_refs_key_consistent(config) {
            report.failures.push("CleanRefsKeyConsistent");
        }
        match mode {
            InvariantMode::StrictLog => {
                if !self.dirty_q_ordered() {
                    report.failures.push("DirtyQOrdered");
                }
                if !self.flushed_ordered() {
                    report.failures.push("FlushedOrdered");
                }
                if !self.queued_after_flushed() {
                    report.failures.push("QueuedAfterFlushed");
                }
                if !self.all_ids_lt_next_write() {
                    report.failures.push("AllIdsLtNextWrite");
                }
            }
            InvariantMode::CoalescedMap => {
                if !self.all_ids_lt_next_write() {
                    report.failures.push("AllIdsLtNextWrite");
                }
            }
        }
        if !self.durable_matches_flushed(config) {
            report.failures.push("DurableMatchesFlushed");
        }
        if !self.no_lost_dirty(config) {
            report.failures.push("NoLostDirty");
        }
        if self.bad_read {
            report.failures.push("NoBadRead");
        }

        report
    }

    #[cfg(not(feature = "creusot"))]
    pub fn check_invariants(&self, config: ModelConfig) -> InvariantReport {
        self.check_invariants_for_mode(config, InvariantMode::StrictLog)
    }

    #[cfg_attr(feature = "creusot", requires(crate::proofs::valid_config(config)))]
    #[cfg_attr(feature = "creusot", ensures(result == crate::proofs::shape_ok(*self, config)))]
    pub fn type_ok(&self, config: ModelConfig) -> bool {
        if self.visible.len() != config.key_count
            || self.write_store.len() != config.max_write
            || self.write_hist.len() != config.max_write
            || self.cache_store.len() != config.max_cache
            || self.durable.len() != config.key_count
            || self.created_dirty.len() != config.max_write
            || self.next_write > config.max_write
            || self.next_cache > config.max_cache
        {
            return false;
        }

        let mut index = 0;
        while index < self.visible.len() {
            match self.visible[index] {
                VisibleRef::None => {}
                VisibleRef::Dirty(id) if config.valid_write(id) => {}
                VisibleRef::Clean(id) if config.valid_cache(id) => {}
                _ => return false,
            }
            index += 1;
        }

        index = 0;
        while index < self.write_store.len() {
            if !self.valid_dirty_record(config, &self.write_store[index]) {
                return false;
            }
            index += 1;
        }

        index = 0;
        while index < self.write_hist.len() {
            if !self.valid_dirty_record(config, &self.write_hist[index]) {
                return false;
            }
            index += 1;
        }

        index = 0;
        while index < self.cache_store.len() {
            if !self.valid_clean_record(config, &self.cache_store[index]) {
                return false;
            }
            index += 1;
        }

        index = 0;
        while index < self.durable.len() {
            if !self.valid_durable_value(config, &self.durable[index]) {
                return false;
            }
            index += 1;
        }

        index = 0;
        while index < self.dirty_q.len() {
            if !config.valid_write(self.dirty_q[index]) {
                return false;
            }
            index += 1;
        }

        index = 0;
        while index < self.flushed.len() {
            if !config.valid_write(self.flushed[index]) {
                return false;
            }
            index += 1;
        }

        true
    }

    #[cfg_attr(feature = "creusot", requires(crate::proofs::valid_config(config)))]
    #[cfg_attr(feature = "creusot", ensures(result == crate::proofs::dirty_refs_live(*self, config)))]
    pub fn dirty_refs_live(&self, config: ModelConfig) -> bool {
        let mut index = 0;
        while index < self.visible.len() {
            if let VisibleRef::Dirty(id) = self.visible[index]
                && (!config.valid_write(id)
                    || !self.write_store[id].present
                    || self.write_store[id].id != Some(id)
                    || self.write_store[id].key != index)
            {
                return false;
            }
            index += 1;
        }
        true
    }

    #[cfg_attr(feature = "creusot", requires(crate::proofs::valid_config(config)))]
    #[cfg_attr(feature = "creusot", ensures(result == crate::proofs::clean_refs_key_consistent(*self, config)))]
    pub fn clean_refs_key_consistent(&self, config: ModelConfig) -> bool {
        let mut index = 0;
        while index < self.visible.len() {
            if let VisibleRef::Clean(id) = self.visible[index]
                && config.valid_cache(id)
                && self.cache_store[id].present
                && self.cache_store[id].key != index
            {
                return false;
            }
            index += 1;
        }
        true
    }

    #[cfg_attr(feature = "creusot", ensures(result == crate::proofs::queued_after_flushed(*self)))]
    pub fn queued_after_flushed(&self) -> bool {
        for &f in &self.flushed {
            for &q in &self.dirty_q {
                if f >= q {
                    return false;
                }
            }
        }
        true
    }

    #[cfg_attr(feature = "creusot", ensures(result == crate::proofs::all_ids_lt_next_write(*self)))]
    pub fn all_ids_lt_next_write(&self) -> bool {
        for &id in &self.dirty_q {
            if id >= self.next_write {
                return false;
            }
        }
        for &id in &self.flushed {
            if id >= self.next_write {
                return false;
            }
        }
        true
    }

    #[cfg_attr(feature = "creusot", requires(crate::proofs::valid_config(config)))]
    #[cfg_attr(feature = "creusot", ensures(result == crate::proofs::durable_matches_flushed(*self, config)))]
    pub fn durable_matches_flushed(&self, config: ModelConfig) -> bool {
        self.durable == apply_flushed(config, &self.flushed, &self.write_hist)
    }

    #[cfg_attr(feature = "creusot", requires(crate::proofs::valid_config(config)))]
    #[cfg_attr(feature = "creusot", ensures(result == crate::proofs::no_lost_dirty(*self, config)))]
    pub fn no_lost_dirty(&self, config: ModelConfig) -> bool {
        let _ = config;
        if self.crashed {
            return true;
        }
        let mut id = 0;
        while id < self.created_dirty.len() {
            if self.created_dirty[id]
                && !contains_id(&self.dirty_q, id)
                && !contains_id(&self.flushed, id)
                && !contains_visible_dirty(&self.visible, id)
                && !self.write_store[id].present
            {
                return false;
            }
            id += 1;
        }
        true
    }

    #[cfg_attr(feature = "creusot", ensures(result == crate::proofs::dirty_q_ordered(*self)))]
    pub fn dirty_q_ordered(&self) -> bool {
        strictly_increasing(&self.dirty_q)
    }

    #[cfg_attr(feature = "creusot", ensures(result == crate::proofs::flushed_ordered(*self)))]
    pub fn flushed_ordered(&self) -> bool {
        strictly_increasing(&self.flushed)
    }

    pub(crate) fn check_key(&self, config: ModelConfig, key: KeyId) -> Result<(), ModelError> {
        if config.valid_key(key) {
            Ok(())
        } else {
            Err(ModelError::InvalidKey(key))
        }
    }

    pub(crate) fn check_key_value(
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

    pub(crate) fn check_write(&self, config: ModelConfig, id: WriteId) -> Result<(), ModelError> {
        if config.valid_write(id) {
            Ok(())
        } else {
            Err(ModelError::InvalidWriteId(id))
        }
    }

    pub(crate) fn check_cache(&self, config: ModelConfig, id: CacheId) -> Result<(), ModelError> {
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

fn contains_id(items: &[usize], needle: usize) -> bool {
    let mut index = 0;
    while index < items.len() {
        if items[index] == needle {
            return true;
        }
        index += 1;
    }
    false
}

fn strictly_increasing(items: &[usize]) -> bool {
    let mut index = 1;
    while index < items.len() {
        if items[index - 1] >= items[index] {
            return false;
        }
        index += 1;
    }
    true
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
