#![cfg(feature = "creusot")]

use creusot_contracts::prelude::*;

use crate::{CacheId, KeyId, ModelConfig, ModelError, ModelState, ValueId, WriteId};

#[logic]
pub fn valid_config(cfg: ModelConfig) -> bool {
    pearlite! {
        cfg.key_count > 0usize &&
        cfg.value_count > 0usize &&
        cfg.max_write > 0usize &&
        cfg.max_cache > 0usize
    }
}

#[requires(valid_config(cfg))]
#[ensures(result@ == state.type_ok(cfg))]
pub fn type_ok(state: &ModelState, cfg: ModelConfig) -> bool {
    state.type_ok(cfg)
}

#[requires(valid_config(cfg))]
#[ensures(result@ == state.dirty_refs_live(cfg))]
pub fn dirty_refs_live(state: &ModelState, cfg: ModelConfig) -> bool {
    state.dirty_refs_live(cfg)
}

#[requires(valid_config(cfg))]
#[ensures(result@ == state.durable_matches_flushed(cfg))]
pub fn durable_matches_flushed(state: &ModelState, cfg: ModelConfig) -> bool {
    state.durable_matches_flushed(cfg)
}

#[requires(valid_config(cfg))]
#[ensures(result@ == state.no_lost_dirty())]
pub fn no_lost_dirty(state: &ModelState, cfg: ModelConfig) -> bool {
    let _ = cfg;
    state.no_lost_dirty()
}

#[requires(valid_config(cfg))]
#[ensures(result@ == state.check_invariants(cfg).is_ok())]
pub fn invariants_hold(state: &ModelState, cfg: ModelConfig) -> bool {
    state.check_invariants(cfg).is_ok()
}

#[requires(valid_config(cfg))]
#[requires(state.check_invariants(cfg).is_ok())]
#[ensures(state.check_invariants(cfg).is_ok())]
pub fn writer_write_step(
    state: &mut ModelState,
    cfg: ModelConfig,
    key: KeyId,
    value: ValueId,
) -> Result<(), ModelError> {
    state.writer_write(cfg, key, value)
}

#[requires(valid_config(cfg))]
#[requires(state.check_invariants(cfg).is_ok())]
#[ensures(state.check_invariants(cfg).is_ok())]
pub fn flusher_flush_next_step(state: &mut ModelState, cfg: ModelConfig) -> Result<(), ModelError> {
    state.flusher_flush_next(cfg)
}

#[requires(valid_config(cfg))]
#[requires(state.check_invariants(cfg).is_ok())]
#[ensures(state.check_invariants(cfg).is_ok())]
pub fn flusher_drop_step(
    state: &mut ModelState,
    cfg: ModelConfig,
    id: WriteId,
) -> Result<(), ModelError> {
    state.flusher_drop(cfg, id)
}

#[requires(valid_config(cfg))]
#[requires(state.check_invariants(cfg).is_ok())]
#[ensures(state.check_invariants(cfg).is_ok())]
pub fn cache_insert_step(
    state: &mut ModelState,
    cfg: ModelConfig,
    key: KeyId,
) -> Result<(), ModelError> {
    state.cache_insert(cfg, key)
}

#[requires(valid_config(cfg))]
#[requires(state.check_invariants(cfg).is_ok())]
#[ensures(state.check_invariants(cfg).is_ok())]
pub fn cache_drop_record_step(
    state: &mut ModelState,
    cfg: ModelConfig,
    id: CacheId,
) -> Result<(), ModelError> {
    state.cache_drop_record(cfg, id)
}

#[requires(valid_config(cfg))]
#[requires(state.check_invariants(cfg).is_ok())]
#[ensures(state.check_invariants(cfg).is_ok())]
pub fn cache_cleanup_visible_step(
    state: &mut ModelState,
    cfg: ModelConfig,
    id: CacheId,
) -> Result<(), ModelError> {
    state.cache_cleanup_visible(cfg, id)
}

#[requires(valid_config(cfg))]
#[requires(state.check_invariants(cfg).is_ok())]
#[ensures(state.check_invariants(cfg).is_ok())]
pub fn reader_step(state: &mut ModelState, cfg: ModelConfig, key: KeyId) -> Result<(), ModelError> {
    state.reader_step(cfg, key)
}

#[requires(valid_config(cfg))]
#[requires(state.check_invariants(cfg).is_ok())]
#[ensures(state.check_invariants(cfg).is_ok())]
pub fn crash_step(state: &mut ModelState, cfg: ModelConfig) -> Result<(), ModelError> {
    state.crash(cfg)
}
