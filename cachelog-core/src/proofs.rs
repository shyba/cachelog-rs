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

#[logic(open)]
pub fn dirty_refs_live(state: ModelState, cfg: ModelConfig) -> bool {
    pearlite! {
        forall<k: Int> 0 <= k && k < cfg.key_count@ ==>
            match state.visible[k] {
                VisibleRef::Dirty(id) =>
                    id < cfg.max_write &&
                    state.write_store[id].present &&
                    state.write_store[id].id == Some(id) &&
                    state.write_store[id].key@ == k,
                _ => true,
            }
    }
}

#[logic(open)]
pub fn clean_refs_key_consistent(state: ModelState, cfg: ModelConfig) -> bool {
    pearlite! {
        forall<k: Int> 0 <= k && k < cfg.key_count@ ==>
            match state.visible[k] {
                VisibleRef::Clean(id) =>
                    id < cfg.max_cache && state.cache_store[id].present ==>
                        state.cache_store[id].key@ == k,
                _ => true,
            }
    }
}

#[logic(open)]
pub fn dirty_q_ordered(state: ModelState) -> bool {
    pearlite! {
        forall<i: Int, j: Int>
            0 <= i && i < j && j < state.dirty_q@.len() ==>
                state.dirty_q[i] < state.dirty_q[j]
    }
}

#[logic(open)]
pub fn flushed_ordered(state: ModelState) -> bool {
    pearlite! {
        forall<i: Int, j: Int>
            0 <= i && i < j && j < state.flushed@.len() ==>
                state.flushed[i] < state.flushed[j]
    }
}

#[logic(open)]
pub fn queued_after_flushed(state: ModelState) -> bool {
    pearlite! {
        forall<i: Int, j: Int>
            0 <= i && i < state.flushed@.len() &&
            0 <= j && j < state.dirty_q@.len() ==>
                state.flushed[i] < state.dirty_q[j]
    }
}

#[logic(open)]
pub fn all_ids_lt_next_write(state: ModelState) -> bool {
    pearlite! {
        (forall<i: Int> 0 <= i && i < state.dirty_q@.len() ==>
            state.dirty_q[i] < state.next_write) &&
        (forall<i: Int> 0 <= i && i < state.flushed@.len() ==>
            state.flushed[i] < state.next_write)
    }
}

#[logic(open)]
pub fn no_lost_dirty(state: ModelState, cfg: ModelConfig) -> bool {
    pearlite! {
        state.crashed ||
        forall<id: Int> 0 <= id && id < cfg.max_write@ &&
            state.created_dirty[id] ==>
                (exists<i: Int> 0 <= i && i < state.dirty_q@.len() && state.dirty_q[i]@ == id) ||
                (exists<i: Int> 0 <= i && i < state.flushed@.len() && state.flushed[i]@ == id) ||
                (exists<k: Int> 0 <= k && k < cfg.key_count@ && match state.visible[k] { VisibleRef::Dirty(d) => d@ == id, _ => false }) ||
                state.write_store[id].present
    }
}

#[logic(open)]
pub fn no_bad_read(state: ModelState) -> bool {
    pearlite! { !state.bad_read }
}

#[logic(open)]
pub fn durable_matches_flushed(state: ModelState, cfg: ModelConfig) -> bool {
    pearlite! {
        state.durable@.len() == cfg.key_count@ &&
        forall<k: Int> 0 <= k && k < cfg.key_count@ ==>
            state.durable[k] == apply_flushed_at(state.flushed@, state.write_hist@, k)
    }
}

#[logic(open)]
#[variant(flushed.len())]
pub fn apply_flushed_at(flushed: Seq<usize>, write_hist: Seq<DirtyRecord>, k: Int) -> DurableValue {
    pearlite! {
        if flushed.len() == 0 {
            DurableValue { present: false, value: 0usize, seq: None }
        } else {
            let last_idx = flushed.len() - 1;
            let id = flushed[last_idx];
            let rest = flushed.subsequence(0, last_idx);
            if write_hist[id@].present && write_hist[id@].key@ == k {
                DurableValue { present: true, value: write_hist[id@].value, seq: Some(id) }
            } else {
                apply_flushed_at(rest, write_hist, k)
            }
        }
    }
}

#[requires(valid_config(cfg))]
#[requires(shape_ok(*state, cfg))]
#[ensures(result == durable_matches_flushed(*state, cfg))]
pub fn durable_replay_equivalence(state: &ModelState, cfg: ModelConfig) -> bool {
    state.durable_matches_flushed(cfg)
}

#[logic(open)]
pub fn inv(state: ModelState, cfg: ModelConfig) -> bool {
    pearlite! {
        shape_ok(state, cfg) &&
        dirty_refs_live(state, cfg) &&
        clean_refs_key_consistent(state, cfg) &&
        dirty_q_ordered(state) &&
        flushed_ordered(state) &&
        queued_after_flushed(state) &&
        all_ids_lt_next_write(state) &&
        no_lost_dirty(state, cfg) &&
        no_bad_read(state) &&
        durable_matches_flushed(state, cfg)
    }
}

#[requires(valid_config(cfg))]
#[ensures(result == shape_ok(*state, cfg))]
pub fn type_ok_holds(state: &ModelState, cfg: ModelConfig) -> bool {
    state.type_ok(cfg)
}

#[requires(valid_config(cfg))]
#[ensures(result == dirty_refs_live(*state, cfg))]
pub fn dirty_refs_live_holds(state: &ModelState, cfg: ModelConfig) -> bool {
    state.dirty_refs_live(cfg)
}

#[requires(valid_config(cfg))]
#[ensures(result == clean_refs_key_consistent(*state, cfg))]
pub fn clean_refs_key_consistent_holds(state: &ModelState, cfg: ModelConfig) -> bool {
    state.clean_refs_key_consistent(cfg)
}

#[ensures(result == dirty_q_ordered(*state))]
pub fn dirty_q_ordered_holds(state: &ModelState) -> bool {
    state.dirty_q_ordered()
}

#[ensures(result == flushed_ordered(*state))]
pub fn flushed_ordered_holds(state: &ModelState) -> bool {
    state.flushed_ordered()
}

#[ensures(result == queued_after_flushed(*state))]
pub fn queued_after_flushed_holds(state: &ModelState) -> bool {
    state.queued_after_flushed()
}

#[ensures(result == all_ids_lt_next_write(*state))]
pub fn all_ids_lt_next_write_holds(state: &ModelState) -> bool {
    state.all_ids_lt_next_write()
}

#[requires(valid_config(cfg))]
#[ensures(result == durable_matches_flushed(*state, cfg))]
pub fn durable_matches_flushed_holds(state: &ModelState, cfg: ModelConfig) -> bool {
    state.durable_matches_flushed(cfg)
}

#[requires(valid_config(cfg))]
#[ensures(result == no_lost_dirty(*state, cfg))]
pub fn no_lost_dirty_holds(state: &ModelState, cfg: ModelConfig) -> bool {
    state.no_lost_dirty(cfg)
}

#[ensures(result == no_bad_read(*state))]
pub fn no_bad_read_holds(state: &ModelState) -> bool {
    !state.bad_read
}

#[requires(valid_config(cfg))]
#[ensures(result == inv(*state, cfg))]
pub fn invariants_hold_holds(state: &ModelState, cfg: ModelConfig) -> bool {
    state.type_ok(cfg)
        && state.dirty_refs_live(cfg)
        && state.clean_refs_key_consistent(cfg)
        && state.dirty_q_ordered()
        && state.flushed_ordered()
        && state.queued_after_flushed()
        && state.all_ids_lt_next_write()
        && state.durable_matches_flushed(cfg)
        && state.no_lost_dirty(cfg)
        && !state.bad_read
}

#[requires(valid_config(cfg))]
#[requires(inv(*state, cfg))]
#[requires(key < cfg.key_count)]
#[requires(value < cfg.value_count)]
#[ensures(inv(^state, cfg))]
pub fn writer_write_step(
    state: &mut ModelState,
    cfg: ModelConfig,
    key: KeyId,
    value: ValueId,
) -> Result<(), ModelError> {
    state.writer_write(cfg, key, value)
}

#[requires(valid_config(cfg))]
#[requires(inv(*state, cfg))]
#[ensures(inv(^state, cfg))]
pub fn flusher_flush_next_step(state: &mut ModelState, cfg: ModelConfig) -> Result<(), ModelError> {
    state.flusher_flush_next(cfg)
}

#[requires(valid_config(cfg))]
#[requires(inv(*state, cfg))]
#[requires(id < cfg.max_write)]
#[ensures(inv(^state, cfg))]
pub fn flusher_drop_step(
    state: &mut ModelState,
    cfg: ModelConfig,
    id: WriteId,
) -> Result<(), ModelError> {
    state.flusher_drop(cfg, id)
}

#[requires(valid_config(cfg))]
#[requires(inv(*state, cfg))]
#[requires(key < cfg.key_count)]
#[ensures(inv(^state, cfg))]
pub fn cache_insert_step(
    state: &mut ModelState,
    cfg: ModelConfig,
    key: KeyId,
) -> Result<(), ModelError> {
    state.cache_insert(cfg, key)
}

#[requires(valid_config(cfg))]
#[requires(inv(*state, cfg))]
#[requires(id < cfg.max_cache)]
#[ensures(inv(^state, cfg))]
pub fn cache_drop_record_step(
    state: &mut ModelState,
    cfg: ModelConfig,
    id: CacheId,
) -> Result<(), ModelError> {
    state.cache_drop_record(cfg, id)
}

#[requires(valid_config(cfg))]
#[requires(inv(*state, cfg))]
#[requires(id < cfg.max_cache)]
#[ensures(inv(^state, cfg))]
pub fn cache_cleanup_visible_step(
    state: &mut ModelState,
    cfg: ModelConfig,
    id: CacheId,
) -> Result<(), ModelError> {
    state.cache_cleanup_visible(cfg, id)
}

#[requires(valid_config(cfg))]
#[requires(inv(*state, cfg))]
#[requires(key < cfg.key_count)]
#[ensures(inv(^state, cfg))]
pub fn reader_step(state: &mut ModelState, cfg: ModelConfig, key: KeyId) -> Result<(), ModelError> {
    state.reader_step(cfg, key)
}

#[requires(valid_config(cfg))]
#[requires(inv(*state, cfg))]
#[ensures(inv(^state, cfg))]
pub fn crash_step(state: &mut ModelState, cfg: ModelConfig) -> Result<(), ModelError> {
    state.crash(cfg)
}

#[requires(valid_config(cfg))]
#[ensures(inv(result, cfg))]
pub fn init_proof(cfg: ModelConfig) -> ModelState {
    ModelState::new(cfg)
}
