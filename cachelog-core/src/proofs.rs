#![cfg(feature = "creusot")]

use creusot_std::prelude::*;

use crate::{
    CacheId, CleanRecord, DurableValue, KeyId, ModelConfig, ModelError, ModelState, ValueId,
    VisibleRef, WriteId,
};
use crate::model::DirtyRecord;

#[logic(open)]
pub fn valid_config(cfg: ModelConfig) -> bool {
    pearlite! {
        cfg.key_count > 0usize &&
        cfg.value_count > 0usize &&
        cfg.max_write > 0usize &&
        cfg.max_cache > 0usize
    }
}

#[logic(open)]
pub fn visible_ref_ok(visible: VisibleRef, cfg: ModelConfig) -> bool {
    pearlite! {
        match visible {
            VisibleRef::None => true,
            VisibleRef::Dirty(id) => id < cfg.max_write,
            VisibleRef::Clean(id) => id < cfg.max_cache,
        }
    }
}

#[logic(open)]
pub fn dirty_record_ok(record: DirtyRecord, cfg: ModelConfig) -> bool {
    pearlite! {
        if !record.present {
            true
        } else {
            record.id != None &&
            match record.id {
                Some(id) =>
                    id < cfg.max_write &&
                    record.key < cfg.key_count &&
                    record.value < cfg.value_count,
                None => false,
            }
        }
    }
}

#[logic(open)]
pub fn clean_record_ok(record: CleanRecord, cfg: ModelConfig) -> bool {
    pearlite! {
        if !record.present {
            true
        } else {
            record.id != None &&
            match record.id {
                Some(id) =>
                    id < cfg.max_cache &&
                    record.key < cfg.key_count &&
                    record.value < cfg.value_count,
                None => false,
            }
        }
    }
}

#[logic(open)]
pub fn durable_value_ok(value: DurableValue, cfg: ModelConfig) -> bool {
    pearlite! {
        if !value.present {
            true
        } else {
            value.seq != None &&
            match value.seq {
                Some(seq) => seq < cfg.max_write && value.value < cfg.value_count,
                None => false,
            }
        }
    }
}

#[logic(open)]
pub fn shape_ok(state: ModelState, cfg: ModelConfig) -> bool {
    pearlite! {
        state.visible@.len() == cfg.key_count@ &&
        state.write_store@.len() == cfg.max_write@ &&
        state.write_hist@.len() == cfg.max_write@ &&
        state.cache_store@.len() == cfg.max_cache@ &&
        state.durable@.len() == cfg.key_count@ &&
        state.created_dirty@.len() == cfg.max_write@ &&
        state.next_write <= cfg.max_write &&
        state.next_cache <= cfg.max_cache &&
        forall<i: Int> 0 <= i && i < state.visible@.len() ==>
            visible_ref_ok(state.visible[i], cfg) &&
        forall<i: Int> 0 <= i && i < state.write_store@.len() ==>
            dirty_record_ok(state.write_store[i], cfg) &&
        forall<i: Int> 0 <= i && i < state.write_hist@.len() ==>
            dirty_record_ok(state.write_hist[i], cfg) &&
        forall<i: Int> 0 <= i && i < state.cache_store@.len() ==>
            clean_record_ok(state.cache_store[i], cfg) &&
        forall<i: Int> 0 <= i && i < state.durable@.len() ==>
            durable_value_ok(state.durable[i], cfg) &&
        forall<i: Int> 0 <= i && i < state.dirty_q@.len() ==>
            state.dirty_q[i] < cfg.max_write &&
        forall<i: Int> 0 <= i && i < state.flushed@.len() ==>
            state.flushed[i] < cfg.max_write
    }
}

#[requires(valid_config(cfg))]
#[ensures(result == shape_ok(*state, cfg))]
pub fn type_ok_holds(state: &ModelState, cfg: ModelConfig) -> bool {
    state.type_ok(cfg)
}

#[requires(valid_config(cfg))]
pub fn dirty_refs_live_holds(state: &ModelState, cfg: ModelConfig) -> bool {
    state.dirty_refs_live(cfg)
}

#[requires(valid_config(cfg))]
pub fn durable_matches_flushed_holds(state: &ModelState, cfg: ModelConfig) -> bool {
    state.durable_matches_flushed(cfg)
}

#[requires(valid_config(cfg))]
pub fn no_lost_dirty_holds(state: &ModelState, cfg: ModelConfig) -> bool {
    let _ = cfg;
    state.no_lost_dirty()
}

#[requires(valid_config(cfg))]
pub fn invariants_hold_holds(state: &ModelState, cfg: ModelConfig) -> bool {
    state.check_invariants(cfg).is_ok()
}

#[requires(valid_config(cfg))]
#[requires(shape_ok(*state, cfg))]
#[requires(key < cfg.key_count)]
#[requires(value < cfg.value_count)]
#[ensures(shape_ok(^state, cfg))]
pub fn writer_write_step(
    state: &mut ModelState,
    cfg: ModelConfig,
    key: KeyId,
    value: ValueId,
) -> Result<(), ModelError> {
    state.writer_write(cfg, key, value)
}

#[requires(valid_config(cfg))]
#[requires(shape_ok(*state, cfg))]
#[ensures(shape_ok(^state, cfg))]
pub fn flusher_flush_next_step(state: &mut ModelState, cfg: ModelConfig) -> Result<(), ModelError> {
    state.flusher_flush_next(cfg)
}

#[requires(valid_config(cfg))]
#[requires(shape_ok(*state, cfg))]
#[requires(id < cfg.max_write)]
#[ensures(shape_ok(^state, cfg))]
pub fn flusher_drop_step(
    state: &mut ModelState,
    cfg: ModelConfig,
    id: WriteId,
) -> Result<(), ModelError> {
    state.flusher_drop(cfg, id)
}

#[requires(valid_config(cfg))]
#[requires(shape_ok(*state, cfg))]
#[requires(key < cfg.key_count)]
#[ensures(shape_ok(^state, cfg))]
pub fn cache_insert_step(
    state: &mut ModelState,
    cfg: ModelConfig,
    key: KeyId,
) -> Result<(), ModelError> {
    state.cache_insert(cfg, key)
}

#[requires(valid_config(cfg))]
#[requires(shape_ok(*state, cfg))]
#[requires(id < cfg.max_cache)]
#[ensures(shape_ok(^state, cfg))]
pub fn cache_drop_record_step(
    state: &mut ModelState,
    cfg: ModelConfig,
    id: CacheId,
) -> Result<(), ModelError> {
    state.cache_drop_record(cfg, id)
}

#[requires(valid_config(cfg))]
#[requires(shape_ok(*state, cfg))]
#[requires(id < cfg.max_cache)]
#[ensures(shape_ok(^state, cfg))]
pub fn cache_cleanup_visible_step(
    state: &mut ModelState,
    cfg: ModelConfig,
    id: CacheId,
) -> Result<(), ModelError> {
    state.cache_cleanup_visible(cfg, id)
}

#[requires(valid_config(cfg))]
#[requires(shape_ok(*state, cfg))]
#[requires(key < cfg.key_count)]
#[ensures(shape_ok(^state, cfg))]
pub fn reader_step(state: &mut ModelState, cfg: ModelConfig, key: KeyId) -> Result<(), ModelError> {
    state.reader_step(cfg, key)
}

#[requires(valid_config(cfg))]
#[requires(shape_ok(*state, cfg))]
#[ensures(shape_ok(^state, cfg))]
pub fn crash_step(state: &mut ModelState, cfg: ModelConfig) -> Result<(), ModelError> {
    state.crash(cfg)
}
