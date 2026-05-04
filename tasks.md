# CacheLog Cleanup Plan

This is the current handoff plan for small agents with no prior conversation
context. It was refreshed after checking the current working tree.

Repo root: `/home/user/repos/tableflip-rs`.

Do not edit `aidocs/*`.

## Checked State

- `cargo check --workspace --all-targets --all-features` passes.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` passes.
- Full final gate passes: `cargo test -q`, `cargo test -q --all-features`, `cargo test -q --features loom`, `cargo test --doc`, `cargo check --benches --features dev-tools`, dev-tools hotpath smoke, standalone model crate tests, model-cachelog all-feature tests, model-cachelog clippy, and `git diff --check`.
- Phase 1 map split is complete:
  - `src/map.rs` is now 925 lines.
  - `src/map/api.rs` contains product API methods.
  - `src/map/types.rs` contains shared public/internal map types.
  - `src/map/debug.rs` contains debug snapshots.
  - `src/map/low_level.rs` contains `LowLevelMap` wrappers.
  - `src/map/coalesced.rs` contains coalesced internals.
  - `src/map/strict.rs` contains strict internals.
  - `src/map/engine.rs` contains engine wrapper/adapters.
- The stricter audit command still fails only because of target-specific dependency false positives:
  `cargo clippy --workspace --all-targets --all-features -- -D warnings -W dead_code -W unused_crate_dependencies -W clippy::too_many_lines -W clippy::cognitive_complexity -W clippy::type_complexity`.
- `src/dirty_mode.rs` has already been split into `src/dirty_mode/{shared,native,loom}.rs`.
- `cachelog-core/src/model.rs` and `model-cachelog/src/tla.rs` have already been split.
- `src/bin/coalesced_hotpath.rs` is now 549 lines, not the old 1700+ line experiment dump.
- `tests/cachelog.rs` has been split into focused integration files with helpers in `tests/common/mod.rs`.
- `benches/live.rs` is now a 44-line Criterion wiring file with benchmark families under `benches/live/`.
- `.codex` has been removed from the index and ignored.

## Global Rules

- Pick one task only unless explicitly assigned multiple tasks.
- Make mechanical behavior-preserving changes first; do semantic cleanup only in the task that asks for it.
- Do not remove tests to make refactors pass.
- Do not widen visibility to `pub` unless the item is already a public API.
- Prefer `pub(super)` or `pub(crate)` for split-module plumbing.
- If a task touches loom cfg code, run the loom gate.
- If a task touches write hot paths, run the dev-tools smoke command.
- If a task touches benchmarks, run the bench check.

Standard gate:

```bash
cargo fmt --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -q
cargo test -q --all-features
cargo test --doc
```

Extra gates when relevant:

```bash
cargo test -q --features loom
cargo check --benches --features dev-tools
cargo run --features dev-tools --bin coalesced_hotpath -- serial-quiet 1
```

## Phase 0: Fix Handoff Drift

### Task 0.1: Keep This Plan Truthful — DONE

Files:

- `tasks.md`
- `changes/close-cachelog-open-issues/tasks.md`

Steps:

1. Remove stale claims that `src/map.rs` is already split beyond `engine.rs`.
2. Remove stale claims that `debug_snapshot` moved out of product code.
3. Remove stale claims that `tests/cachelog.rs` is split.
4. Keep the old OpenSpec completion notes only if they explicitly say they are historical.

Definition of done:

- `changes/close-cachelog-open-issues/tasks.md` either matches this current plan or clearly marks older completion notes as historical.
- Search both task files for stale completion claims about `debug_snapshot`, `tests/cachelog.rs`, and completed map splitting; any such claim must be removed or explicitly marked historical.
- Standard gate passes or pre-existing unrelated failures are reported.

## Phase 1: Split `src/map.rs` By Responsibility

This phase is the highest-value cleanup. Keep `src/map.rs` as the parent module
for now and add child modules under `src/map/`. Do not create `src/map/mod.rs`
while `src/map.rs` exists.

### Task 1.1: Move Shared Types To `src/map/types.rs` — DONE

Files:

- `src/map.rs`
- New: `src/map/types.rs`
- `src/lib.rs` only if imports require it.

Move only type definitions and simple inherent impls:

- `CacheLogConfig`
- `DirtyWriteMode`
- `DirtyBacklogCounts`
- `FlushWork`
- `LowLevelMap`
- `VisibleValue`
- `CoalescedInflight`
- `CoalescedRecord`
- type aliases for coalesced maps.

Requirements:

- Do not move method bodies that mutate state.
- Do not change public names or derives.
- Re-export from `src/map.rs` so external imports remain unchanged.

Definition of done:

- `src/map.rs` no longer contains those type definitions.
- Standard gate passes.
- `cargo test -q --features loom` passes.

### Task 1.2: Move Debug Snapshot Code To `src/map/debug.rs` — DONE

Files:

- `src/map.rs`
- New: `src/map/debug.rs`
- `src/lib.rs`
- `tests/loom.rs`
- `tests/loom_exhaustive.rs`
- `tests/loom_support.rs`

Move:

- `DebugRecord`
- `DebugVisible`
- `DebugSnapshot`
- `CacheLogMap::debug_snapshot`
- `LowLevelMap::debug_snapshot`
- model-conformance helper impls tied to debug snapshots.

Requirements:

- Keep exports behind the existing `#[cfg(feature = "loom")]` public boundary where applicable.
- Keep test-only/model-conformance code out of normal product paths when possible.
- Split the 131-line `debug_snapshot` into helper functions:
  `collect_visible`, `collect_strict_pending`, `collect_strict_inflight`,
  `collect_coalesced_pending`, `collect_counters`.

Definition of done:

- `rg -n "fn debug_snapshot|pub struct Debug|pub enum Debug" src/map.rs` returns empty.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings -W clippy::too_many_lines` no longer reports `debug_snapshot`.
- Standard gate passes.
- `cargo test -q --features loom` passes.

### Task 1.3: Move Low-Level API Wrappers To `src/map/low_level.rs` — DONE

Files:

- `src/map.rs`
- New: `src/map/low_level.rs`
- `benches/live.rs`
- `src/bin/coalesced_hotpath.rs`
- `tests/cachelog.rs`

Move:

- `impl LowLevelMap<'_, K, V, H>`
- specialized `LowLevelMap<'_, Vec<u8>, Vec<u8>, H>`
- specialized `LowLevelMap<'_, Vec<u8>, Arc<Vec<u8>>, H>`
- specialized `LowLevelMap<'_, Vec<u8>, Arc<[u8]>, H>`
- specialized `LowLevelMap<'_, Vec<u8>, Bytes, H>`

Requirements:

- Do not move product `CacheLogMap` methods in this task.
- Keep `cachelog::low_level::LowLevelMap` re-export working.
- Keep id-bearing APIs under `map.low_level()`.

Definition of done:

- `rg -n "impl<.*LowLevelMap|pub fn insert_dirty_borrowed|pub fn debug_snapshot" src/map.rs` returns empty or only non-wrapper declarations.
- Standard gate passes.
- `cargo check --benches --features dev-tools` passes.

### Task 1.4: Move Coalesced Internals To `src/map/coalesced.rs` — DONE

Files:

- `src/map.rs`
- New: `src/map/coalesced.rs`
- `src/map/engine.rs`
- `src/perf.rs`
- `tests/cachelog.rs`
- `tests/loom.rs`

Move:

- `CoalescedOverlay` methods.
- `CoalescedCompactScratch`.
- coalesced active/draining/inflight helpers.
- `flush_batch_coalesced`.
- `mark_flushed_coalesced`.
- coalesced read helpers.
- coalesced batch compaction helpers if they are not part of public API.

Requirements:

- Do not change coalesced visibility semantics.
- Preserve `active_publish` locking around writes, flush rotation, clean inserts, and prefix snapshots.
- Preserve latest-value behavior for duplicate keys.
- Preserve current perf counters and cfg behavior.

Definition of done:

- `src/map.rs` no longer contains `CoalescedOverlay` implementation bodies.
- Standard gate passes.
- `cargo test -q --features loom` passes.
- `cargo run --features dev-tools --bin coalesced_hotpath -- serial-quiet 1` passes.

### Task 1.5: Move Strict Internals To `src/map/strict.rs` — DONE

Files:

- `src/map.rs`
- New: `src/map/strict.rs`
- `src/map/engine.rs`
- `src/dirty_mode/native.rs`
- `src/dirty_mode/loom.rs`
- `tests/cachelog.rs`
- `tests/loom.rs`

Move:

- strict dirty publish helpers.
- strict flush/mark helpers.
- `is_strict_flushed`.
- `advance_strict_flushed_upto`.
- strict-specific background flush adapter if it is not already isolated in `engine.rs`.

Requirements:

- Do not alter global strict write id ordering.
- Do not alter `mark_flushed` frontier behavior.
- Do not make strict writes blocking.

Definition of done:

- Strict-specific helpers are not in `src/map.rs`.
- Standard gate passes.
- `cargo test -q --features loom` passes.

### Task 1.6: Move Product API Shell To `src/map/api.rs` — DONE

Files:

- `src/map.rs`
- New: `src/map/api.rs`
- `README.md`

Move product-facing methods:

- constructors.
- `visible_len`, `dirty_log_len`, `dirty_backlog_counts`.
- `contains`, `get_cloned`, `read`.
- prefix/list methods.
- `put`, `put_batch`.
- `insert_clean_if_absent`, `evict_clean`, `cleanup_stale_visible`.
- public background flush start methods.

Requirements:

- Do not expose low-level id APIs directly on `CacheLogMap`.
- Keep docs/examples accurate.
- Keep `src/map.rs` as module wiring plus fields/shared helpers only.

Definition of done:

- `src/map.rs` is small enough to act as a module shell.
- Standard gate passes.
- `cargo test --doc` passes.

## Phase 2: Deduplicate Coalesced Batch Publishing

### Task 2.1: Replace Four Publish Helpers With One Internal Loop — DONE

Files:

- `src/map.rs` or `src/map/coalesced.rs` after Phase 1.
- `tests/cachelog.rs`
- `src/bin/coalesced_hotpath.rs`

Current duplicate functions:

- `publish_coalesced_batch`
- `publish_coalesced_batch_from_scratch`
- `publish_coalesced_batch_without_ids`
- `publish_coalesced_batch_without_ids_from_scratch`

Steps:

1. Add one helper that accepts an iterator or drained vector of `(K, V)`.
2. Make return shaping explicit with a small enum or two thin wrappers:
   one path returns `Vec<WriteId>`, one path returns `usize`.
3. Keep id allocation order identical to current behavior.
4. Keep scratch reuse intact.

Definition of done:

- Only one loop calls `publish_coalesced_active` for batch publishing.
- Duplicate-key tests still pass:
  `borrowed_coalesced_batch_matches_owned_duplicate_semantics`,
  `bytes_batch_coalesces_duplicate_keys`.
- Standard gate passes.
- `cargo run --features dev-tools --bin coalesced_hotpath -- batch-unique-quiet 1000` passes.

## Phase 3: Consolidate Borrowed Byte-Value Helpers

### Task 3.1: Add Generic Borrowed-Value Helper — DONE

Files:

- `src/map.rs` or `src/map/low_level.rs` after Phase 1.
- `benches/live.rs`
- `tests/cachelog.rs`

Current duplicated value types:

- `Vec<u8>`
- `Arc<Vec<u8>>`
- `Arc<[u8]>`
- `Bytes`

Steps:

1. Add a private helper that receives `key: &[u8]`, `value: &[u8]`, and `make_value: impl FnOnce(&[u8]) -> V`.
2. Add a private borrowed-batch helper that receives `entries` and `make_value: impl Fn(&[u8]) -> V`.
3. Keep public specialized low-level methods as thin wrappers.
4. Do not change allocation behavior for `Bytes`; keep `bytes_from_borrowed`.

Definition of done:

- Strict/coalesced mode dispatch is written once for single borrowed writes.
- Strict/coalesced mode dispatch is written once for borrowed batches.
- Public specialized methods still exist unless a separate task removes them.
- Standard gate passes.
- `cargo check --benches --features dev-tools` passes.

## Phase 4: Simplify Background Flush Reconfiguration API

### Task 4.1: Remove Unused Setter-Style Public Methods — DONE

Files:

- `src/background_flush.rs`
- `tests/cachelog.rs`
- `README.md` if it documents these methods.

Current methods:

- `update_policy`
- `set_flush_limit`
- `set_trigger_dirty`
- `reconfigure`
- `reconfigure_for_batch_size`

Steps:

1. Confirm only `reconfigure` and `reconfigure_for_batch_size` are externally used in tests/docs.
2. Make `update_policy` private or merge it into `reconfigure`.
3. Remove `set_flush_limit` and `set_trigger_dirty` unless a real caller exists.
4. Keep behavior: lowering threshold wakes existing backlog.

Definition of done:

- Public reconfiguration surface is `reconfigure` plus `reconfigure_for_batch_size`.
- `rg -n "set_flush_limit|set_trigger_dirty|update_policy" src tests README.md` shows no public unused setter references.
- Standard gate passes.

## Phase 5: Split Tests By Behavior

### Task 5.1: Extract Common Test Helpers — DONE

Files:

- `tests/cachelog.rs`
- New: `tests/common/mod.rs` or `tests/support.rs`

Move:

- `read_pair`
- `read_triplet`
- `PrefixOrderBuildHasher`
- `PrefixOrderHasher`

Definition of done:

- Helpers are not duplicated in new test files.
- `cargo test -q` passes.

### Task 5.2: Split `tests/cachelog.rs` Into Focused Integration Files — DONE

Files:

- `tests/cachelog.rs`
- New files:
  `tests/product_api.rs`,
  `tests/prefix.rs`,
  `tests/strict.rs`,
  `tests/coalesced.rs`,
  `tests/background_flush.rs`,
  `tests/byte_prefix_map.rs`,
  `tests/borrowed_values.rs`.

Rules:

- Move tests by behavior, not by original line ranges.
- Do not change assertions while moving.
- Keep concurrency tests with the behavior they protect.

Definition of done:

- `tests/cachelog.rs` is deleted or reduced to a tiny module harness.
- No single new test file exceeds about 450 lines unless justified.
- `cargo test -q` passes.
- `cargo test -q --all-features` passes.

## Phase 6: Split Benchmarks Without Changing Coverage

### Task 6.1: Move Benchmark Families To Modules — DONE

Files:

- `benches/live.rs`
- New directory: `benches/live/`
- New files:
  `benches/live/write.rs`,
  `benches/live/read.rs`,
  `benches/live/prefix.rs`,
  `benches/live/stale.rs`,
  `benches/live/mixed.rs`,
  `benches/live/bytes.rs`.

Requirements:

- Keep the same Criterion group names and benchmark ids.
- Keep `#[cfg(not(feature = "loom"))]` and `#[cfg(feature = "loom")]` behavior equivalent.
- Do not drop strict/coalesced serial, batch, read, prefix, background, stale, or bytes-value benchmarks.

Definition of done:

- `benches/live.rs` only wires modules and `criterion_group!`.
- `cargo check --benches --features dev-tools` passes.

## Phase 7: BytePrefixMap API Audit

### Task 7.1: Classify And Document Public Methods — DONE

Files:

- `src/byte_prefix_map.rs`
- `DESIGN.md`
- `README.md`

Steps:

1. List every `pub fn` in `BytePrefixMap`.
2. Classify each as product API, trie-preserving flush API, background flush API, or removable mirror.
3. Add short doc comments for methods whose behavior differs from `CacheLogMap`, especially `advance_trie`, `list_prefix`, background flush, and `mark_flushed`.

Definition of done:

- Every public `BytePrefixMap` method has a clear reason to exist.
- Standard gate passes.
- `cargo test --doc` passes.

### Task 7.2: Reduce Wrapper Duplication Only Where Safe — DONE

Files:

- `src/byte_prefix_map.rs`
- `src/map.rs` or split map modules.

Requirements:

- Do not expose raw inner map mutation that bypasses trie maintenance.
- If extracting shared flush helpers, use private helper functions, not a public trait.
- Preserve background flush trie pruning.

Definition of done:

- Repeated flush/mark/persist wrapper logic is smaller.
- BytePrefixMap tests pass.
- Standard gate passes.

## Phase 8: Dev Tools And Repo Artifacts

### Task 8.1: Keep `coalesced_hotpath` Small And Current — DONE

Files:

- `src/bin/coalesced_hotpath.rs`

Steps:

1. Verify the active mode table is the intended current list.
2. Remove any uncalled helper found by `cargo clippy --bin coalesced_hotpath --features dev-tools -- -D warnings`.
3. Do not add historical experiments back to the binary.

Definition of done:

- `cargo clippy --bin coalesced_hotpath --features dev-tools -- -D warnings` passes.
- `cargo run --features dev-tools --bin coalesced_hotpath -- serial-quiet 1` passes.

### Task 8.2: Remove Empty Tracked `.codex` — DONE

Files:

- `.codex`
- `.gitignore` only if needed.

Steps:

1. Confirm `.codex` is an empty tracked file.
2. Remove it from the repo.
3. Ensure real local Codex/session artifacts are ignored if needed.

Definition of done:

- `git ls-files .codex` returns empty.
- Standard gate is not required; this is a repo metadata task.

## Phase 9: Dependency And Audit-Lint Cleanup

### Task 9.1: Resolve Or Document `unused_crate_dependencies` — DONE

Files:

- `Cargo.toml`
- `src/lib.rs`
- `src/sync.rs`
- `benches/live.rs`
- `tests/proptests.rs`

Current audit command reports:

- `arc_swap`
- `crossbeam_queue`
- `cachelog_core`
- `cachelog_model`
- `criterion`
- `proptest`

Rules:

- Do not remove dependencies that are used only under non-loom, benches, or tests without checking those targets.
- Prefer target-specific explanation or lint scoping over dummy imports in production code.
- If a dependency is genuinely unused, remove it and update `Cargo.lock`.

Audit note:

- `cargo clippy --workspace --all-targets --all-features -- -D warnings` is the supported whole-crate gate and passes.
- The stricter audit command adds `-W unused_crate_dependencies`, which does not understand this workspace's target-specific dependency graph well enough to be used as an all-features gate.
- The reported dependencies are all target-scoped or transitive through cfg-gated modules/tests:
  - `arc_swap`: used in `src/sync.rs` for the non-loom shared-arc shim.
  - `crossbeam_queue`: used in `src/sync.rs` for the non-loom bounded queue shim.
  - `cachelog_core`: used by `src/map.rs` model-conformance code under `#[cfg(any(test, feature = "dev-tools"))]`.
  - `cachelog_model`: used by `src/map.rs` model-conformance code under `#[cfg(any(test, feature = "dev-tools"))]`.
  - `criterion`: used by `benches/live.rs` behind the `dev-tools` bench target.
  - `proptest`: used by `tests/proptests.rs` under the test target.

Definition of done:

- Either the audit command no longer reports unused dependencies, or `tasks.md` documents why this lint is not a valid all-features gate for this workspace.
- Normal clippy gate still passes.

## Phase 10: Final Verification

### Task 10.1: Whole Cleanup Gate — DONE

Run:

```bash
cargo fmt --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -q
cargo test -q --all-features
cargo test -q --features loom
cargo test --doc
cargo check --benches --features dev-tools
cargo run --features dev-tools --bin coalesced_hotpath -- serial-quiet 1
cargo test -q --manifest-path cachelog-core/Cargo.toml --features serde
cargo test -q --manifest-path model-cachelog/Cargo.toml --all-features
cargo clippy --manifest-path model-cachelog/Cargo.toml --all-targets --all-features -- -D warnings
git diff --check
```

Definition of done:

- All commands pass.
- Final response lists remaining deliberate debt, if any.
- No `aidocs/*` files changed.
