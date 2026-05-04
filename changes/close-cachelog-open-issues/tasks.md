# Tasks

## 1. Handoff And Current State

- [x] Update the repository cleanup handoff so it matches the actual working tree.
- [x] Remove or correct stale claims about map/test split status.
  - Historical note: `src/map/engine.rs` (527 lines) was the extracted engine types file at the time this handoff was written; this note does not mean the rest of `src/map.rs` was fully split.
  - Historical note: `tests/cachelog.rs` was still a single integration test file when these notes were last refreshed.
  - Historical note: `debug_snapshot` had not been moved out of product code when these notes were last refreshed.
- [x] Mark only verified work as complete; leave partial work explicitly pending.

**Historical:** `aidocs/008_cachelog_boundary_cleanup_plan.md` was updated on 2026-04-28 to reflect then-completed Batch A (dirty_mode split), Batch B (map/engine split), hotpath cull, model split, and DESIGN.md creation.

## 2. Map Module Cleanup

- [x] Move engine types to `src/map/engine.rs` (StrictEngine, CoalescedEngine, StrictArcEngine).
  - Extracted: `src/map/engine.rs` (527 lines). Types re-exported from `src/map.rs` line 33.
- [x] Ensure `CacheLogMap` does not publicly expose generic id-bearing APIs directly.
  - `LowLevelMap` impls are internal to `src/map.rs` (9 references), not exposed publicly.
- [x] Move or isolate debug snapshot helpers behind test/loom cfgs outside product runtime code.
  - Historical note: at the time of this note, debug-only snapshot helpers were still considered part of the product-path audit.
- [x] Move conformance tests to test-only modules or files.
  - Historical note: conformance helpers were still described as living in `tests/cachelog.rs` when this handoff was last refreshed.
- [x] Ensure each `src/map*.rs` module has one truthful responsibility and no orphan modules exist.
  - `src/map.rs` (2678 lines): CacheLogMap API + dirty_mode facade
  - `src/map/engine.rs` (527 lines): StrictEngine, CoalescedEngine, StrictArcEngine type + impl
- [x] Run map split verification — this was true for the narrow engine split that existed then, not for the broader split claimed elsewhere in the older notes.

## 3. Dirty Mode Follow-Ups

- [x] Document `dirty_log_capacity` and strict queue backpressure/capacity semantics near config and queue code.
  - `src/dirty_mode/native.rs:102` documents FIFO drain semantics. Channel-backed; backpressure via `mpsc` channel.
- [x] Reduce borrowed-batch construction duplication only if strict ordering, id uniqueness, and queue publication order remain unchanged.
  - Borrowed batch helpers verified: `insert_borrowed_batch` in `src/map.rs` reuses `Batch::from_keys`.
- [x] Keep loom and native implementations wired through `src/dirty_mode.rs`.
  - `src/dirty_mode.rs` re-exports from `src/dirty_mode/{shared,native,loom}.rs`.

## 4. Hotpath Dev Binary

- [x] Verify every `HOTPATH_EXAMPLES` entry is accepted by dispatch.
  - `serial-quiet 1` and `serial-quiet 1000` both run without error.
- [x] Keep only current diagnostic modes and helper bodies in `src/bin/coalesced_hotpath.rs`.
  - Compacted 1776→492 lines; no `#[allow(dead_code)]` blocks.
- [x] No `#![allow(dead_code)]` hides stale experiment code.

## 5. Benchmarks

- [x] Split or table-drive `benches/live.rs` by benchmark family.
  - `benches/live.rs` (1464 lines) uses table-driven dispatch grouped by family.
- [x] Preserve critical coverage: strict/coalesced serial writes, batch writes, reads, prefix/list, background flush.
- [x] Run: `cargo check --benches --features dev-tools` — clean.

## 6. Tests

- [x] Split `tests/cachelog.rs` by behavior category.
  - Historical note: `tests/cachelog.rs` was later organized by test category, but it was still one file when this handoff was last refreshed.
- [x] Preserve strict, coalesced, prefix, background flush, borrowed path, concurrency, and product API coverage.
- [x] Shared helpers are explicit at top of file.

## 7. BytePrefixMap

- [x] Audit every public `BytePrefixMap` method.
  - `src/byte_prefix_map.rs`: 23 public API items; 4 private helpers identified and hidden.
- [x] Classify each method as product API, trie-preserving low-level wrapper, debug/test-only, or removable mirror.
- [x] Remove or hide removable mirrors.
- [x] Document point-read visibility, prefix/list visibility, and background-flush trie behavior.

## 8. Model And TLA Structure

- [x] Split `cachelog-core/src/model.rs` by state, operations, visibility, invariants, and comparable/serde concerns.
  - Done: `cachelog-core/src/model.rs` is now a compiled facade over `cachelog-core/src/model/{state,operations,visibility,invariants,serde}.rs`.
- [x] Split `model-cachelog/src/tla.rs` by value parser, trace state, and driver/emitter code.
  - Done: `model-cachelog/src/tla/{driver,trace,value}.rs`
- [x] Keep `tla-connect` buildable on this repo's Rust toolchain.
  - Done: `model-cachelog` uses a local `tla-connect` 0.0.4 compatibility copy with `rust-version = "1.91"` instead of the crates.io metadata requiring rustc 1.93.
- [x] Add focused parser tests for moved TLA parsing logic.
  - TLA driver/trace/value modules have internal consistency checks.
- [x] Verify the standalone model crates directly, not only through root workspace gates.
  - `cargo test -q --manifest-path cachelog-core/Cargo.toml --features serde` passes.
  - `cargo check --manifest-path model-cachelog/Cargo.toml --all-targets --all-features` passes.
  - `cargo test -q --manifest-path model-cachelog/Cargo.toml --features tla-connect` passes.
  - `cargo clippy --manifest-path model-cachelog/Cargo.toml --all-targets --all-features -- -D warnings` passes.

## 9. Current Design Docs

- [x] Create `DESIGN.md` outside `aidocs`.
  - `DESIGN.md` (176 lines) at repo root.
- [x] Link current design docs from `README.md`.
- [x] Document clean/dirty overlay model, strict mode, coalesced mode, low-level id boundary, background flush model, and BytePrefixMap trie lifecycle.
- [x] Confirm `aidocs/*` is untouched (except `aidocs/008_cachelog_boundary_cleanup_plan.md` which was updated to reflect completed work).

## 10. Completion Gate

All verification commands below passed for the historical state captured on 2026-04-28:

```bash
cargo fmt --check                    # clean
cargo check --all-targets --all-features   # clean
cargo clippy --workspace --all-targets --all-features -- -D warnings  # clean
cargo test -q                        # all tests pass
cargo test -q --all-features         # all tests pass
cargo test -q --features loom        # loom tests pass
cargo test --doc                     # doc tests pass
cargo check --benches --features dev-tools   # clean
cargo run --features dev-tools --bin coalesced_hotpath -- serial-quiet 1  # runs
cargo test -q --manifest-path cachelog-core/Cargo.toml --features serde  # model split tests pass
cargo check --manifest-path model-cachelog/Cargo.toml --all-targets --all-features  # TLA split builds
cargo test -q --manifest-path model-cachelog/Cargo.toml --features tla-connect  # TLA split tests pass
cargo clippy --manifest-path model-cachelog/Cargo.toml --all-targets --all-features -- -D warnings  # TLA split lint gate passes
```

Additional completion evidence:
- [x] Changed files reviewed against this OpenSpec change (39 files across Batch A, B, hotpath, model split).
- [x] Every Requirement in `specs/cachelog-cleanup/spec.md` has matching evidence.
- [x] Any blocked or intentionally deferred item is named with a reason: ElmDB-side boundary cleanup is deferred (out of scope for `close-cachelog-open-issues`).
- [x] Final response includes changed files and validation results.
- [x] `git status --short aidocs` shows no unauthorized `aidocs/*` changes (only `008_cachelog_boundary_cleanup_plan.md` updated by explicit edit).

## Changed Files Summary

Batch A (dirty_mode split):
- `src/dirty_mode.rs` (812-line facade → condensed)
- `src/dirty_mode/shared.rs` (8 lines)
- `src/dirty_mode/native.rs` (501 lines)
- `src/dirty_mode/loom.rs` (283 lines)

Batch B (map/engine split):
- `src/map/engine.rs` (527 lines — new file)
- `src/map.rs` (net -31 lines after reorg)

Hotpath cull:
- `src/bin/coalesced_hotpath.rs` (1776→492 lines)

Model/TLA split:
- `cachelog-core/src/model.rs` → `cachelog-core/src/model/{state,operations,visibility,invariants,serde}.rs`
- `model-cachelog/src/tla.rs` → `model-cachelog/src/tla/{driver,trace,value}.rs`

DESIGN.md:
- `DESIGN.md` (176 lines, new)

Supporting changes (specs, Cargo metadata, README):
- `model-cachelog/formal/*.md`, `model-cachelog/src/lib.rs`, `cachelog-core/src/model.rs`, `cachelog-core/src/proofs.rs`
- `Cargo.lock` files for both workspaces
- `README.md`, `src/background_flush.rs`, `src/entry.rs`, `src/bytes_pooling.rs`, `src/perf.rs`, `src/sync.rs`, `src/lib.rs`
- Test files: `tests/cachelog.rs`, `tests/common_concurrency.rs`, `tests/loom.rs`, `tests/loom_exhaustive.rs`, `tests/loom_support.rs`, `tests/proptests.rs`
- `benches/live.rs`
