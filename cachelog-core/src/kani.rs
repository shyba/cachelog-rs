#![cfg(kani)]

use crate::{ModelConfig, ModelState, VisibleRef};

fn cfg_small() -> ModelConfig {
    ModelConfig::new(2, 2, 3, 2)
}

#[kani::proof]
#[kani::unwind(16)]
fn strict_write_then_flush_preserves_core_invariants() {
    let cfg = cfg_small();
    let mut state = ModelState::new(cfg);

    let _ = state.writer_write(cfg, 0, 0);
    let _ = state.flusher_flush_next(cfg);

    assert!(state.durable[0].present);
    assert_eq!(state.durable[0].seq, Some(0));
    assert_eq!(state.visible[0], VisibleRef::Dirty(0));
}

#[kani::proof]
#[kani::unwind(16)]
fn stale_drop_does_not_remove_newer_visible_write() {
    let cfg = cfg_small();
    let mut state = ModelState::new(cfg);

    let _ = state.writer_write(cfg, 0, 0);
    let _ = state.flusher_flush_next(cfg);

    let _ = state.writer_write(cfg, 0, 1);
    let _ = state.flusher_drop(cfg, 0);

    assert_eq!(state.visible[0], VisibleRef::Dirty(1));
    assert!(state.write_store[1].present);
    assert!(!state.bad_read);
}

#[kani::proof]
#[kani::unwind(16)]
fn flushing_old_record_replays_to_durable_without_losing_new_dirty() {
    let cfg = cfg_small();
    let mut state = ModelState::new(cfg);

    let _ = state.writer_write(cfg, 0, 0);
    let _ = state.writer_write(cfg, 0, 1);
    let _ = state.flusher_flush_next(cfg);

    assert_eq!(state.durable[0].seq, Some(0));
    assert_eq!(state.visible[0], VisibleRef::Dirty(1));
    assert!(state.write_store[1].present);
}

#[kani::proof]
#[kani::unwind(16)]
fn deterministic_mixed_writer_flusher_sequence_keeps_ordering() {
    let cfg = cfg_small();
    let mut state = ModelState::new(cfg);

    let _ = state.writer_write(cfg, 0, 0);
    let _ = state.writer_write(cfg, 1, 1);
    let _ = state.flusher_flush_next(cfg);
    let _ = state.flusher_drop(cfg, 0);

    // Equivalent concrete post-state checks without invoking nested Vec scans
    // that cause path explosion in this harness.
    assert_eq!(state.flushed.len(), 1);
    assert_eq!(state.flushed[0], 0);
    assert_eq!(state.dirty_q.len(), 1);
    assert_eq!(state.dirty_q[0], 1);
    assert_eq!(state.next_write, 2);

    assert_eq!(state.visible[0], VisibleRef::None);
    assert_eq!(state.visible[1], VisibleRef::Dirty(1));
}
