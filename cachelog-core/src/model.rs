mod invariants;
mod operations;
mod serde;
mod state;
mod visibility;

pub use self::serde::{InvariantMode, InvariantReport, ModelError, ModelStep};

#[cfg(not(creusot))]
pub use self::serde::{
    ComparableCleanRecord, ComparableDirtyRecord, ComparableDurableValue, ComparableState,
    ComparableVisibleRef,
};

#[cfg(feature = "creusot")]
pub use self::state::DirtyRecord;

pub use self::state::{
    CacheId, CleanRecord, DurableValue, KeyId, ModelConfig, ModelState, ValueId, VisibleRef, WriteId,
};

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
    fn flusher_drop_of_old_dirty_does_not_clear_newer_visible_dirty_ref() {
        let cfg = ModelConfig::new(2, 3, 4, 2);
        let mut state = ModelState::new(cfg);

        state.writer_write(cfg, 0, 1).unwrap();
        state.writer_write(cfg, 0, 2).unwrap();
        state.flusher_flush_next(cfg).unwrap();
        state.flusher_drop(cfg, 0).unwrap();

        assert_eq!(state.visible[0], VisibleRef::Dirty(1));
        assert!(!state.write_store[0].present);
        assert!(state.write_store[1].present);
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

    #[test]
    fn clean_refs_key_consistent_detects_mismatch() {
        let cfg = ModelConfig::new(2, 2, 3, 2);
        let mut state = ModelState::new(cfg);

        state.writer_write(cfg, 0, 1).unwrap();
        state.flusher_flush_next(cfg).unwrap();
        state.flusher_drop(cfg, 0).unwrap();
        state.cache_insert(cfg, 0).unwrap();
        assert!(state.clean_refs_key_consistent(cfg));

        state.cache_store[0].key = 1;
        assert!(!state.clean_refs_key_consistent(cfg));
    }

    #[test]
    fn dirty_refs_live_detects_key_mismatch() {
        let cfg = ModelConfig::new(2, 2, 3, 2);
        let mut state = ModelState::new(cfg);

        state.writer_write(cfg, 0, 1).unwrap();
        assert!(state.dirty_refs_live(cfg));

        state.write_store[0].key = 1;
        assert!(!state.dirty_refs_live(cfg));
    }

    #[test]
    fn queued_after_flushed_detects_violation() {
        let cfg = ModelConfig::new(2, 2, 4, 2);
        let mut state = ModelState::new(cfg);

        state.writer_write(cfg, 0, 0).unwrap();
        state.writer_write(cfg, 1, 1).unwrap();
        state.flusher_flush_next(cfg).unwrap();
        assert!(state.queued_after_flushed());

        state.dirty_q.insert(0, 0);
        assert!(!state.queued_after_flushed());
    }

    #[test]
    fn all_ids_lt_next_write_detects_violation() {
        let cfg = ModelConfig::new(2, 2, 4, 2);
        let mut state = ModelState::new(cfg);

        state.writer_write(cfg, 0, 0).unwrap();
        assert!(state.all_ids_lt_next_write());

        state.dirty_q.push(99);
        assert!(!state.all_ids_lt_next_write());
    }

    #[test]
    fn invalid_inputs_report_errors() {
        let cfg = ModelConfig::tla_small();
        let mut state = ModelState::new(cfg);

        assert_eq!(state.writer_write(cfg, 10, 0), Err(ModelError::InvalidKey(10)));
        assert_eq!(
            state.writer_write(cfg, 0, 10),
            Err(ModelError::InvalidValue(10))
        );
        assert_eq!(
            state.flusher_drop(cfg, 10),
            Err(ModelError::InvalidWriteId(10))
        );
        assert_eq!(
            state.cache_drop_record(cfg, 10),
            Err(ModelError::InvalidCacheId(10))
        );
    }

    #[test]
    fn comparable_state_is_deterministic() {
        let cfg = ModelConfig::tla_small();
        let mut state = ModelState::new(cfg);

        state.writer_write(cfg, 0, 1).unwrap();
        state.flusher_flush_next(cfg).unwrap();
        state.cache_insert(cfg, 0).unwrap();

        let comparable = state.comparable();
        assert_eq!(comparable.next_write, 1);
        assert!(comparable.write_hist.contains_key(&0));
        assert!(comparable.durable.contains_key(&0));
    }
}
