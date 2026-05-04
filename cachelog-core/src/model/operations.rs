//! Model state operations (state transitions / step application).

#[cfg(feature = "creusot")]
use creusot_std::macros::{ensures, requires};

use super::state::{CleanRecord, DirtyRecord, DurableValue, ModelConfig, ModelState, VisibleRef};
use super::serde::{ModelError, ModelStep};
use super::state::{CacheId, KeyId, ValueId, WriteId};

impl ModelState {
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

    #[cfg_attr(feature = "creusot", requires(crate::proofs::valid_config(config)))]
    #[cfg_attr(feature = "creusot", requires(crate::proofs::inv(*self, config)))]
    #[cfg_attr(feature = "creusot", requires(key < config.key_count))]
    #[cfg_attr(feature = "creusot", requires(value < config.value_count))]
    #[cfg_attr(feature = "creusot", ensures(crate::proofs::inv(^self, config)))]
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

    #[cfg_attr(feature = "creusot", requires(crate::proofs::valid_config(config)))]
    #[cfg_attr(feature = "creusot", requires(crate::proofs::inv(*self, config)))]
    #[cfg_attr(feature = "creusot", ensures(crate::proofs::inv(^self, config)))]
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

    #[cfg_attr(feature = "creusot", requires(crate::proofs::valid_config(config)))]
    #[cfg_attr(feature = "creusot", requires(crate::proofs::inv(*self, config)))]
    #[cfg_attr(feature = "creusot", requires(id < config.max_write))]
    #[cfg_attr(feature = "creusot", ensures(crate::proofs::inv(^self, config)))]
    pub fn flusher_drop(&mut self, config: ModelConfig, id: WriteId) -> Result<(), ModelError> {
        self.check_write(config, id)?;
        if self.crashed {
            return Ok(());
        }
        if contains_id(&self.flushed, id) && self.write_store[id].present {
            self.write_store[id] = DirtyRecord::absent();
            for visible in &mut self.visible {
                if *visible == VisibleRef::Dirty(id) {
                    *visible = VisibleRef::None;
                }
            }
        }
        Ok(())
    }

    #[cfg_attr(feature = "creusot", requires(crate::proofs::valid_config(config)))]
    #[cfg_attr(feature = "creusot", requires(crate::proofs::inv(*self, config)))]
    #[cfg_attr(feature = "creusot", requires(key < config.key_count))]
    #[cfg_attr(feature = "creusot", ensures(crate::proofs::inv(^self, config)))]
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

    #[cfg_attr(feature = "creusot", requires(crate::proofs::valid_config(config)))]
    #[cfg_attr(feature = "creusot", requires(crate::proofs::inv(*self, config)))]
    #[cfg_attr(feature = "creusot", requires(id < config.max_cache))]
    #[cfg_attr(feature = "creusot", ensures(crate::proofs::inv(^self, config)))]
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

    #[cfg_attr(feature = "creusot", requires(crate::proofs::valid_config(config)))]
    #[cfg_attr(feature = "creusot", requires(crate::proofs::inv(*self, config)))]
    #[cfg_attr(feature = "creusot", requires(id < config.max_cache))]
    #[cfg_attr(feature = "creusot", ensures(crate::proofs::inv(^self, config)))]
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

    #[cfg_attr(feature = "creusot", requires(crate::proofs::valid_config(config)))]
    #[cfg_attr(feature = "creusot", requires(crate::proofs::inv(*self, config)))]
    #[cfg_attr(feature = "creusot", requires(key < config.key_count))]
    #[cfg_attr(feature = "creusot", ensures(crate::proofs::inv(^self, config)))]
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

    #[cfg_attr(feature = "creusot", requires(crate::proofs::valid_config(config)))]
    #[cfg_attr(feature = "creusot", requires(crate::proofs::inv(*self, config)))]
    #[cfg_attr(feature = "creusot", ensures(crate::proofs::inv(^self, config)))]
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
