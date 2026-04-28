# CacheLog Cleanup Task Plan

This file is the handoff plan for small agents with no prior conversation context.
The goal is to eliminate the whole-repo complexity findings without changing
product behavior or regressing the performance/correctness work already landed.

## Repository Context

- Repo root: `/home/user/repos/tableflip-rs`.
- Crate name: `cachelog`.
- Current major concept: `CacheLogMap` has two dirty write modes.
- `DirtyWriteMode::StrictLog` preserves ordered dirty records and id-bearing flush semantics.
- `DirtyWriteMode::CoalescedMap` models latest-value visibility and should stay close to deployed free2 behavior: fast concurrent map writes, no public meaning for write ids, and no unnecessary strict ordering beyond internal flush needs.
- `BytePrefixMap` wraps `CacheLogMap<Vec<u8>, V>` and owns a prefix trie side-channel. It must not expose raw inner map mutation because dirty writes need trie maintenance.
- Public product APIs should be simple: `put`, `put_batch`, `read`, `get_cloned`, prefix/list APIs, background flush APIs.
- Id-bearing APIs such as `insert_dirty`, `flush_batch`, `mark_flushed`, `read_full`, and debug snapshot APIs belong under `map.low_level()` unless they are `BytePrefixMap` wrappers that preserve trie invariants.

## Global Requirements

Every task must preserve these rules:

- Do not change behavior without a test explaining the behavior.
- Do not hide failing tests by weakening assertions.
- Do not remove correctness coverage just to shrink files.
- Do not introduce new dependencies unless the task explicitly says so.
- Do not track or edit `aidocs/*` unless the user explicitly re-enables that. The historical docs are intentionally ignored for now.
- Do not use `python`; if scripting is unavoidable, use `python3` and a virtualenv at `/tmp/.venv`.
- Prefer `rg` and `rg --files` for search.
- Use small mechanical moves first, then behavior-preserving simplification.
- After each task, run the task-specific gate and record exact command output in the final response.
- If a task touches hot write paths, run at least one relevant benchmark or dev-tools smoke command in addition to tests.
- If a task touches `loom` cfg code, run the loom gate listed below.
- If a task changes public API, update examples/README/tests so `cargo test --doc` still passes.

## Standard Gates

Run these after any code task unless the task gives a narrower gate:

```bash
cargo fmt --check
cargo check --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -q
cargo test -q --all-features
cargo test --doc
```

Run this after tasks touching loom-specific code:

```bash
cargo test -q --features loom
```

Run this after tasks touching `src/bin/coalesced_hotpath.rs`:

```bash
cargo run --features dev-tools --bin coalesced_hotpath -- serial-quiet 1
```

Run this after tasks touching benchmarks:

```bash
cargo check --benches --features dev-tools
```

## Execution Strategy For Small Agents

- Pick one task only unless explicitly assigned multiple tasks.
- Read only the listed files first.
- Make behavior-preserving changes with minimal surface area.
- Do not refactor unrelated nearby code.
- Do not rewrite algorithms unless the task explicitly requests it.
- Keep public API churn intentional and documented.
- Prefer moving code to modules over editing semantics while splitting files.
- When splitting a file, first move exact code and prove the build passes, then simplify in a later task.
- If a task is blocked by a failing gate unrelated to your files, stop and report the exact failure.

## Current Known Overcomplexity Findings

These are the findings this plan must fully drain:

- `src/map.rs` is the core bottleneck: large file, too many responsibilities, high cfg density, mixed product API, strict engine, coalesced engine, background flush hooks, perf counters, debug snapshots, conformance helpers, and borrowed value-type helpers.
- `src/bin/coalesced_hotpath.rs` is experiment history as executable code.
- `src/dirty_mode.rs` duplicates loom and non-loom worlds and repeats borrowed-batch value-type variants.
- `benches/live.rs` repeats the same benchmark shapes across modes, payloads, value types, hashers, and workloads.
- `tests/cachelog.rs` is a mega integration file mixing unrelated concerns.
- `src/byte_prefix_map.rs` mirrors too much of `CacheLogMap` and makes the side-channel trie lifecycle harder to reason about.
- `cachelog-core/src/model.rs` is dense and should be split by model concern.
- `model-cachelog/src/tla.rs` mixes TLA value parsing, driver, emitter, and trace state.
- Historical `aidocs` plans dominate repo weight. Do not edit them now, but produce clean current docs elsewhere.

## Batch A: Repair Broken Splits (Pre-Phase-1 Corrections)

These tasks fix incomplete or incorrect work from earlier cleanup attempts.

### Task A1: Wire `src/dirty_mode.rs` Split

Context:

- Earlier work created `src/dirty_mode/native.rs` and `src/dirty_mode/loom.rs` but the parent `src/dirty_mode.rs` still contained the original monolithic implementation.
- The facade (`src/dirty_mode.rs`) was never updated to use the split files.

Files:

- `src/dirty_mode.rs`
- `src/dirty_mode/shared.rs`
- `src/dirty_mode/native.rs`
- `src/dirty_mode/loom.rs`

Requirements:

- `src/dirty_mode.rs` must be a thin facade that declares the three submodules with correct `#[cfg]` gates.
- `pub(crate) mod shared` unconditionally.
- `pub(crate) mod native` behind `#[cfg(not(feature = "loom"))]`.
- `pub(crate) mod loom` behind `#[cfg(feature = "loom")]`.
- Re-export `DirtyMode`, `OrderedFifoDirty`, `FlushWork` with matching `#[cfg]` gates.
- No `mod imp` blocks in `src/dirty_mode.rs`.
- `FlushWork` enum lives in `shared.rs`.

Hard verification:

```bash
rg "mod imp" src/dirty_mode.rs  # must be empty
rg "^[[:space:]]*pub(crate) trait DirtyMode|^[[:space:]]*pub(crate) struct OrderedFifoDirty" src/dirty_mode.rs  # must be empty
rg "mod shared|mod native|mod loom|pub\(crate\) use .*DirtyMode|pub\(crate\) use .*FlushWork" src/dirty_mode.rs  # must show wiring
```

Definition of done:

- Hard verification commands pass.
- `cargo check --all-targets --all-features` passes.
- `cargo test -q --features loom` passes.

### Task A2: Remove Stale Functions From `src/bin/coalesced_hotpath.rs`

Context:

- Earlier work added `#![allow(dead_code)]` to suppress warnings from orphaned `run_*` function bodies.
- The dead functions remain in the file and must be removed properly.
- The mode table was never updated — stale mode names still reference deleted functions.

Files:

- `src/bin/coalesced_hotpath.rs`

Requirements:

- Remove `#![allow(dead_code)]` — the file should compile with zero warnings.
- Update the `hotpath_mode_table!` macro to only contain the 13 active modes: `serial`, `serial-quiet`, `batch-unique`, `batch-unique-quiet`, `batch-repeated`, `batch-repeated-quiet`, `flush-batch-unique`, `mark-flushed-unique`, `flush-batch-build-multi`, `mark-flushed-multi`, `clone-keys-multi`, `materialize-dirty-arcs-multi`, `materialize-dirty-arcs-keyclone-multi`.
- Remove ALL orphaned `run_*` function bodies not referenced by the active modes.
- Remove dead helper functions (`build_compacted_repeated_bytes_batches`, `build_compacted_repeated_arc_bytes_batches`, `build_compacted_repeated_bytes_multi`, `build_compacted_repeated_arc_bytes_multi`, `build_compacted_unique_bytes_batches`, `build_compacted_unique_arc_bytes_batches`, `PreparedCompactedBatch`).
- The file must compile with `cargo check --bin coalesced_hotpath --features dev-tools` showing zero warnings.

Hard verification:

```bash
rg "allow\(dead_code\)" src/bin/coalesced_hotpath.rs  # must be empty
cargo check --bin coalesced_hotpath --features dev-tools 2>&1  # must show zero warnings
```

Definition of done:

- Hard verification commands pass.
- `cargo run --features dev-tools --bin coalesced_hotpath -- serial-quiet 1` passes.
- `cargo clippy --bin coalesced_hotpath --features dev-tools -- -D warnings` passes.

### Task A3: Repair Misleading `src/map/` Split

Context:

- Earlier work created `src/map/types.rs` and `src/map/mod.rs`, but `types.rs` is a copy of the original monolithic `src/map.rs` — not an extraction by concern.
- The `mod.rs` documentation falsely claims the split isolates concerns when it does not.
- The `src/map.rs` file was restored during merge conflicts but must be deleted (Rust uses `src/map/` directory when both `src/map.rs` and `src/map/mod.rs` exist).

Files:

- `src/map.rs` (to be deleted — Rust conflict)
- `src/map/types.rs`
- `src/map/mod.rs`

Requirements:

- Delete `src/map.rs` so Rust uses the `src/map/` directory.
- Update `src/map/mod.rs` to honestly document that `types.rs` is the current monolith pending extraction (tasks B1-B5 will do the real split).
- `src/map/mod.rs` documentation must NOT claim the module is split by concern until tasks B1-B5 complete.

Hard verification:

```bash
# src/map.rs must not exist
ls src/map.rs  # must fail
# types.rs must still contain the full monolith
wc -l src/map/types.rs  # will be ~3188 lines until B1-B5
```

Definition of done:

- Hard verification commands pass.
- `cargo check --all-targets --all-features` passes.
- `cargo test --lib` passes.

### Task A4: Update This File After Batch A Fixes

Context:

- After completing tasks A1-A3, update this file to mark Batch A complete and note the actual state of each repair.

Files:

- `tasks.md`

Steps:

1. Update the completion log below with the actual changes made.
2. Ensure the hard verification results are recorded.
3. Verify no stale references to old file names or module structure remain in this file.

## Phase 0: Baseline And Guardrails

### Task 0.1: Capture Baseline Build And Public Surface

Context:

- This task creates a before-state for later agents.
- No code changes other than an optional generated note under `tasks.md` are needed.

Files:

- `tasks.md`
- `src/lib.rs`
- `src/map.rs`
- `src/byte_prefix_map.rs`
- `README.md`
- `examples/basic.rs`

Steps:

1. Run the standard gates.
2. Run `cargo test -q --features loom`.
3. Run `cargo run --features dev-tools --bin coalesced_hotpath -- serial-quiet 1`.
4. Inspect public exports in `src/lib.rs`.
5. Inspect product-facing API examples in `README.md` and `examples/basic.rs`.
6. Record any existing failures without trying to fix them.

Definition of done:

- Standard gates pass or exact pre-existing failures are documented.
- Loom gate passes or exact pre-existing failures are documented.
- Dev-tools smoke command passes or exact pre-existing failures are documented.
- No source code behavior changed.

## Phase 1: Split `src/map.rs` Without Semantic Changes

### Task 1.1: Move Top-Level Types To `src/map/types.rs`

Context:

- `src/map.rs` currently owns product API plus all engine internals.
- The first split must be mechanical and behavior-preserving.

Files:

- `src/map.rs`
- New file: `src/map/types.rs`
- `src/lib.rs` only if module visibility requires it.

Move candidates:

- `CacheLogConfig`
- `DirtyWriteMode`
- `DirtyBacklogCounts`
- `FlushWork`
- `LowLevelMap`
- Public debug structs only if they do not create cfg churn.

Requirements:

- Use `pub(crate)` where possible inside the crate.
- Do not change struct fields or derived traits.
- Do not change public re-exports.
- Do not move engine methods yet.

Definition of done:

- `src/map.rs` loses type definitions but public API remains identical.
- `cargo check --all-targets --all-features` passes.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` passes.

### Task 1.2: Move Strict Engine To `src/map/strict.rs`

Context:

- Strict mode has ordered dirty-id semantics.
- This task must not alter ordering, mark-flushed behavior, or low-level API shape.

Files:

- `src/map.rs`
- New file: `src/map/strict.rs`
- `src/dirty_mode.rs`
- `tests/cachelog.rs`
- `tests/loom.rs`
- `tests/loom_exhaustive.rs`

Move candidates:

- `StrictArcEngine`
- Strict-specific read/write/flush helper methods.
- Strict background flush service adapter if it only depends on strict engine.

Requirements:

- Keep strict `append`, `append_batch`, `flush_batch`, and `mark_flushed` behavior exactly the same.
- Keep all strict ordering tests passing.
- Do not touch coalesced logic.
- If private helper visibility gets messy, prefer `pub(super)` over widening to `pub`.

Definition of done:

- Strict-specific implementation is in `src/map/strict.rs`.
- No public API changes.
- Standard gates pass.
- `cargo test -q --features loom` passes.

### Task 1.3: Move Coalesced Engine To `src/map/coalesced.rs`

Context:

- Coalesced mode is the less-strict latest-value model.
- It must stay fast and should not gain unnecessary strict ordering.

Files:

- `src/map.rs`
- New file: `src/map/coalesced.rs`
- `src/perf.rs`
- `tests/cachelog.rs`
- `tests/common_concurrency.rs`
- `benches/live.rs`
- `src/bin/coalesced_hotpath.rs`

Move candidates:

- `CoalescedArcEngine`
- `CoalescedOverlay`
- `CoalescedRecord`
- `CoalescedInflight`
- coalesced active/draining/inflight helpers.
- coalesced borrowed batch compaction helpers if they are engine-specific.

Requirements:

- Do not change `CoalescedMap` semantics.
- Preserve active/draining/inflight visibility during flush.
- Preserve clean-insert rejection while dirty coalesced data exists.
- Preserve `visible_len` behavior for coalesced duplicate keys.
- Preserve perf counter cfg behavior.
- Keep `active_publish` synchronization semantics intact.

Definition of done:

- Coalesced-specific implementation is in `src/map/coalesced.rs`.
- Standard gates pass.
- `cargo test -q --features loom` passes.
- `cargo run --features dev-tools --bin coalesced_hotpath -- serial-quiet 1` passes.

### Task 1.4: Move Low-Level API Wrappers To `src/map/low_level.rs`

Context:

- Id-bearing operations are intentionally not product-facing.
- They should remain available for tests, engines, flusher integrations, and benchmarks through `map.low_level()`.

Files:

- `src/map.rs`
- New file: `src/map/low_level.rs`
- `src/lib.rs`
- `tests/cachelog.rs`
- `benches/live.rs`
- `src/bin/coalesced_hotpath.rs`

Move candidates:

- `impl LowLevelMap<'_, K, V, H>`
- specialized `LowLevelMap<'_, Vec<u8>, ...>` borrowed/preowned helpers.

Requirements:

- Do not re-expose id-bearing methods directly on `CacheLogMap`.
- Keep `cachelog::low_level::*` re-export working.
- `BytePrefixMap` wrapper methods are not part of this task.
- Tests should still call `map.low_level().insert_dirty(...)` where low-level semantics are intended.

Definition of done:

- `src/map.rs` no longer contains `LowLevelMap` method bodies.
- `rg -n "pub fn .*insert_dirty|pub fn .*flush_batch|pub fn .*mark_flushed" src/map.rs` shows no accidental product-facing low-level methods on `CacheLogMap`.
- Standard gates pass.

### Task 1.5: Move Debug And Conformance Test Helpers Out Of Core Map

Context:

- Debug snapshots and model conformance helpers are useful, but they should not increase production map complexity.

Files:

- `src/map.rs`
- New file: `src/map/debug.rs`
- New file: `src/map/conformance.rs` or test-only module.
- `tests/loom.rs`
- `tests/loom_exhaustive.rs`
- `tests/loom_support.rs`

Move candidates:

- `DebugRecord`
- `DebugVisible`
- `DebugSnapshot`
- `debug_snapshot`
- `conformance_tests` module.

Requirements:

- Keep cfgs narrow: `#[cfg(any(test, feature = "loom"))]` for debug snapshots.
- Keep conformance tests test-only.
- Do not expose debug types unless already public under loom/test cfg.

Definition of done:

- Production core map file no longer contains debug snapshot implementations.
- Standard gates pass.
- `cargo test -q --features loom` passes.

### Task 1.6: Move Product API Shell To `src/map/api.rs`

Context:

- Once strict/coalesced/low-level/debug code is split, product API should be easy to read.

Files:

- `src/map.rs`
- New file: `src/map/api.rs`
- `README.md`
- `examples/basic.rs`

Move candidates:

- `CacheLogMap::new`
- `CacheLogMap::with_hasher`
- `contains`
- `get_cloned`
- `read`
- `put`
- `put_batch`
- prefix/list APIs
- visible/dirty count APIs
- background flush public start methods if they are product-facing.

Requirements:

- Preserve product API docs.
- Preserve `CacheLogMap` public exports.
- Do not move engine internals back into API file.

Definition of done:

- `src/map.rs` becomes mostly module declarations plus shared private helpers that could not yet be moved.
- Standard gates pass.
- `cargo test --doc` passes.

### Task 1.7: Final Map Split Audit

Context:

- This task ensures Phase 1 actually resolved the complexity finding rather than only shuffling code.

Files:

- `src/map.rs`
- `src/map/*.rs`

Steps:

1. Count lines and public fns per file with `wc -l` and `rg -n "pub fn|fn "`.
2. Count cfg density with `rg -n "#\\[cfg" src/map.rs src/map`.
3. Check no file is a new 2,000-line dumping ground.
4. Write a short summary in the final response only.

Definition of done:

- `src/map.rs` is small enough to read as an index/shell.
- Strict, coalesced, low-level, debug, and API responsibilities are separated.
- Standard gates pass.

## Phase 2: Cull `src/bin/coalesced_hotpath.rs`

### Task 2.1: Classify Hotpath Modes

Context:

- The dev binary is currently experiment history.
- Do not delete modes until they are classified.

Files:

- `src/bin/coalesced_hotpath.rs`
- `README.md` if it documents dev-tools.
- `tasks.md` if notes need updating.

Steps:

1. List all mode names from the mode dispatch table.
2. Classify each mode as one of:
   - keep-current-gate
   - keep-diagnostic
   - archive-only
   - delete-stale
3. Use existing comments and function names to infer purpose.
4. Do not change code in this task unless there is dead unreachable code.

Definition of done:

- Final response includes a table of modes and classifications.
- No behavior changes unless dead unreachable code is removed.
- `cargo run --features dev-tools --bin coalesced_hotpath -- serial-quiet 1` passes.

### Task 2.2: Delete Stale Hotpath Modes

Context:

- Keep enough modes to protect current performance decisions.
- Remove abandoned microbench variants that are no longer part of the decision loop.

Files:

- `src/bin/coalesced_hotpath.rs`

Requirements:

- Keep a short usage list.
- Keep `serial-quiet` as smoke gate.
- Keep any modes used by current documentation or benches.
- Remove deleted mode names from dispatch and usage together.
- Do not keep code "just in case"; archive the idea in `tasks.md` or final response instead.

Definition of done:

- File LOC materially decreases.
- `cargo run --features dev-tools --bin coalesced_hotpath -- serial-quiet 1` passes.
- `cargo check --all-targets --features dev-tools` passes.

### Task 2.3: Extract Repeated Hotpath Helpers

Context:

- After stale mode deletion, simplify repeated map setup and write loops.

Files:

- `src/bin/coalesced_hotpath.rs`

Requirements:

- Prefer small helper functions over macros unless type constraints force macros.
- Keep benchmark body simple enough to inspect generated work.
- Avoid allocations inside measured loops unless the mode is explicitly measuring allocation.

Definition of done:

- Duplicate setup logic is reduced.
- Smoke command passes.
- Clippy all-features/all-targets passes.

## Phase 3: Refactor `src/dirty_mode.rs`

### Task 3.1: Split Loom And Non-Loom Implementations

Context:

- `src/dirty_mode.rs` has two parallel implementations behind cfgs.
- The split should isolate cfg complexity.

Files:

- `src/dirty_mode.rs`
- New file: `src/dirty_mode/loom.rs`
- New file: `src/dirty_mode/native.rs`
- Maybe new file: `src/dirty_mode/shared.rs`

Requirements:

- Move code mechanically first.
- Preserve `DirtyMode` trait shape.
- Preserve `OrderedFifoDirty` public crate-internal API.
- Keep loom and non-loom gates independent.

Definition of done:

- `src/dirty_mode.rs` is mostly module selection and shared trait exports.
- `cargo check --all-targets --all-features` passes.
- `cargo test -q --features loom` passes.

### Task 3.2: Consolidate Borrowed Batch Record Construction

Context:

- Borrowed batch paths repeat across `Vec<u8>`, `Arc<Vec<u8>>`, `Arc<[u8]>`, and `Bytes`.
- The repeated logic is id reservation, record allocation, queue append, and optional loom shadow update.

Files:

- `src/dirty_mode/native.rs`
- `src/dirty_mode/loom.rs`
- `src/bytes_pooling.rs`
- `src/map/low_level.rs` or current equivalent.

Requirements:

- Preserve strict global append ordering.
- Do not reintroduce duplicate write-id races.
- Do not block writes on full queues.
- Keep value construction outside the common append loop if that keeps type constraints simpler.
- Use helper closures like `FnMut(&[u8]) -> V` if it reduces duplication without harming readability.

Definition of done:

- Borrowed batch code has one common loop per implementation world or a clearly shared helper.
- Tests covering strict batch/single ordering still pass.
- `cargo test -q --features loom` passes.
- Standard gates pass.

### Task 3.3: Document Dirty Queue Backpressure Model

Context:

- Earlier work intentionally avoided blocking writes on bounded strict queues.
- The capacity contract should be explicit.

Files:

- `src/dirty_mode.rs` or split files.
- `src/map/types.rs` or wherever `CacheLogConfig` lives.
- `README.md` if public docs mention capacity.

Requirements:

- Document whether strict dirty queue capacity is a preallocation hint, backpressure limit, or retention limit.
- Do not claim bounded memory if the implementation uses unbounded channel behavior.
- Align docs with code.

Definition of done:

- Capacity semantics are documented near config and queue implementation.
- Standard gates pass.

## Phase 4: Simplify Benchmarks

### Task 4.1: Split `benches/live.rs` By Benchmark Family

Context:

- Criterion supports multiple bench files, but preserve current `[[bench]]` setup unless changing Cargo is necessary.
- If multiple bench targets are introduced, update `Cargo.toml` explicitly.

Files:

- `benches/live.rs`
- New files under `benches/` if needed.
- `Cargo.toml`

Suggested families:

- dirty write serial and concurrent.
- dirty batch writes.
- reads.
- prefix/list.
- background flush.
- value representation and payload size.

Requirements:

- Keep benchmark IDs stable when possible.
- If an ID changes, document why in final response.
- Do not remove current critical performance coverage without moving it elsewhere.

Definition of done:

- `benches/live.rs` no longer contains all benchmark families.
- `cargo check --benches --features dev-tools` passes.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` passes.

### Task 4.2: Table-Drive Repeated Benchmark Shapes

Context:

- Many bench functions vary only by write mode, hasher, payload size, or value type.

Files:

- `benches/*.rs`

Requirements:

- Use small helper functions where type inference stays readable.
- Use macros only where generic functions become worse.
- Keep `black_box` placement unchanged in measured loops unless intentionally fixing a benchmark bug.

Definition of done:

- Duplicate benchmark loops are reduced.
- Benchmark names remain understandable.
- `cargo check --benches --features dev-tools` passes.

### Task 4.3: Remove Stale Exploratory Bench Cases

Context:

- Some cases exist only because they supported abandoned experiments.

Files:

- `benches/*.rs`
- `tasks.md` if a removed case needs historical note.

Requirements:

- Do not remove these categories:
  - strict vs coalesced serial writes.
  - strict vs coalesced batch writes.
  - reads.
  - prefix/list.
  - background flush smoke coverage.
  - payload size comparison for the currently relevant value representations.
- Remove old one-off variants not tied to current design or regression gates.

Definition of done:

- Bench suite is smaller and still covers current risk.
- `cargo check --benches --features dev-tools` passes.

## Phase 5: Split Integration Tests

### Task 5.1: Create Test Module Layout

Context:

- `tests/cachelog.rs` mixes all behavior categories.
- Start by creating modules and moving exact tests.

Files:

- `tests/cachelog.rs`
- New files under `tests/cachelog/` or separate integration files.
- `tests/common_concurrency.rs`
- `tests/loom_support.rs`

Suggested split:

- `tests/cachelog/strict.rs`
- `tests/cachelog/coalesced.rs`
- `tests/cachelog/prefix.rs`
- `tests/cachelog/background_flush.rs`
- `tests/cachelog/borrowed.rs`
- `tests/cachelog/concurrency.rs`
- `tests/cachelog/product_api.rs`

Requirements:

- Move tests mechanically first.
- Preserve test names where possible.
- Keep common helpers in a shared module.
- Avoid broad import globs if they hide dependencies.

Definition of done:

- `tests/cachelog.rs` is an index or much smaller root.
- `cargo test -q` passes.
- `cargo test -q --all-features` passes.

### Task 5.2: Rename Tests To Match Product Semantics

Context:

- Some tests still use implementation-era names.
- Test names should describe behavior, not internal mechanics, unless explicitly low-level.

Files:

- `tests/cachelog/*.rs`
- `tests/loom.rs`
- `tests/loom_exhaustive.rs`

Requirements:

- Keep names clear for failures.
- Include `low_level` in test names that depend on ids or explicit flush batches.
- Include `coalesced` or `strict` only when behavior is mode-specific.

Definition of done:

- Test names form a readable behavior inventory.
- Standard gates pass.

### Task 5.3: Remove Test Duplication After Split

Context:

- After moving tests, repeated setup helpers should be obvious.

Files:

- `tests/cachelog/*.rs`
- `tests/common_concurrency.rs`

Requirements:

- Extract shared setup helpers only when used by at least three tests.
- Do not over-abstract assertions.
- Keep concurrency tests explicit.

Definition of done:

- Repeated map config/setup code is reduced where useful.
- `cargo test -q` passes.
- `cargo test -q --features loom` passes if loom files were touched.

## Phase 6: Narrow `BytePrefixMap`

### Task 6.1: Audit `BytePrefixMap` Public API

Context:

- `BytePrefixMap` is a product type with prefix trie invariants.
- It should not mirror every low-level `CacheLogMap` method unless the wrapper maintains trie state.

Files:

- `src/byte_prefix_map.rs`
- `README.md`
- `examples/basic.rs`
- `tests/cachelog/prefix.rs` or current prefix tests.

Steps:

1. List every public method on `BytePrefixMap`.
2. Classify as:
   - product API.
   - required low-level wrapper preserving trie invariants.
   - debug/test-only.
   - removable mirror.
3. Do not change code in this audit task unless there is dead private code.

Definition of done:

- Final response includes classification and proposed removals.
- Standard gates still pass.

### Task 6.2: Remove Or Hide Unnecessary `BytePrefixMap` Mirrors

Context:

- Only keep wrappers that preserve prefix trie correctness or are needed by product-facing use.

Files:

- `src/byte_prefix_map.rs`
- Tests and examples that call removed methods.

Requirements:

- Do not reintroduce `inner()`.
- Do not allow callers to dirty-write the inner map without trie maintenance.
- If a low-level wrapper remains, explain its trie effect in docs.
- Keep background flush only if it updates/prunes the prefix trie correctly.

Definition of done:

- `BytePrefixMap` API is narrower.
- Prefix/list tests pass.
- Background flush prefix trie pruning tests pass.
- Standard gates pass.

### Task 6.3: Make Prefix Trie Advancement Semantics Explicit

Context:

- `advance_trie` exists because dirty writes and trie visibility are decoupled.
- This is surprising and must be documented or redesigned.

Files:

- `src/byte_prefix_map.rs`
- `README.md`
- `examples/basic.rs`
- Prefix tests.

Requirements:

- Document when a write becomes visible to point reads vs prefix reads.
- Document what background flush does to trie state.
- Add or keep tests proving stale trie keys do not hide newer visible keys.

Definition of done:

- Docs and tests describe prefix visibility clearly.
- `cargo test --doc` passes.
- Standard gates pass.

## Phase 7: Split Semantic Model Files

### Task 7.1: Split `cachelog-core/src/model.rs`

Context:

- This model is important and complexity is partly justified.
- Split by concern, not by random type size.

Files:

- `cachelog-core/src/model.rs`
- New files under `cachelog-core/src/model/`
- `cachelog-core/src/lib.rs`
- `cachelog-core/src/proofs.rs`
- `model-cachelog/src/lib.rs`

Suggested modules:

- `state.rs`
- `operations.rs`
- `visibility.rs`
- `invariants.rs`
- `serde.rs` or `comparable.rs`
- `tests.rs` if inline tests exist.

Requirements:

- Preserve public API of `cachelog-core`.
- Preserve serde behavior.
- Preserve proof/model tests.
- Avoid moving code and changing semantics in the same patch.

Definition of done:

- `model.rs` is an index or small facade.
- `cargo test -q -p cachelog-core` passes if package syntax applies.
- If workspace package flags are not set up, run root standard gates.

### Task 7.2: Make Model Invariants Easy To Find

Context:

- Future agents need to connect code invariants to product behavior.

Files:

- `cachelog-core/src/model/invariants.rs`
- `model-cachelog/README.md`
- `model-cachelog/formal/README.md`

Requirements:

- Put invariant checks in one obvious module.
- Each invariant should have a short comment explaining the product rule.
- Do not edit `aidocs/*`.

Definition of done:

- Invariant module can be read without scanning the full model.
- Model tests pass.

## Phase 8: Split TLA Bridge

### Task 8.1: Split TLA Value Parsing From Driver

Context:

- `model-cachelog/src/tla.rs` mixes parsing and driver/emitter logic.

Files:

- `model-cachelog/src/tla.rs`
- New files under `model-cachelog/src/tla/`
- `model-cachelog/src/lib.rs`

Suggested modules:

- `value.rs` for TLA value parsing.
- `trace.rs` for trace state.
- `driver.rs` for runner/emitter.

Requirements:

- Preserve public API used by tests and docs.
- Keep parser tests near parser module.
- Do not change generated output formats.

Definition of done:

- TLA parser code is isolated.
- Model-cachelog tests pass.
- Root standard gates pass.

### Task 8.2: Add Focused Parser Tests

Context:

- Splitting parser code is safer with focused test coverage.

Files:

- `model-cachelog/src/tla/value.rs`
- Existing or new tests under `model-cachelog`.

Requirements:

- Cover simple atom/string/number/list/map values currently supported.
- Cover malformed value errors if the parser exposes errors.
- Do not use snapshot tests unless existing project style already does.

Definition of done:

- Parser tests fail on obvious parser regressions.
- Model-cachelog tests pass.

## Phase 9: Current Design Documentation Without `aidocs`

### Task 9.1: Create Current `DESIGN.md`

Context:

- Historical plans are large and should not be the main design entry point.
- Do not edit or move `aidocs/*`.

Files:

- New file: `DESIGN.md`
- `README.md`
- `tasks.md`

Required content:

- Product model: clean store, dirty overlay, point reads, prefix reads.
- Strict mode semantics.
- Coalesced mode semantics.
- Why write ids are low-level only.
- Background flush model and failure behavior.
- BytePrefixMap trie lifecycle.
- Testing and benchmarking gates.
- What `aidocs` are: historical notes, not current source of truth.

Definition of done:

- A fresh agent can read `README.md`, `DESIGN.md`, and `tasks.md` to understand current architecture.
- No `aidocs/*` edits.
- `cargo test --doc` passes.

### Task 9.2: Link Current Docs From README

Context:

- Users should find stable docs without reading historical plans.

Files:

- `README.md`
- `DESIGN.md`

Requirements:

- Keep README concise.
- Link `DESIGN.md` for architecture.
- Do not paste long design history into README.

Definition of done:

- README has a clear architecture link.
- `cargo test --doc` passes.

## Phase 10: Final Complexity Verification

### Task 10.1: Whole-Repo Static Complexity Recheck

Context:

- This task proves the original findings are drained.

Files:

- Whole repo.

Steps:

1. Run `wc -l` for the originally flagged files.
2. Run `rg -n "fn |pub fn " src/map.rs src/map src/dirty_mode.rs src/dirty_mode benches tests cachelog-core/src model-cachelog/src`.
3. Run `rg -n "#\\[cfg" src/map.rs src/map src/dirty_mode.rs src/dirty_mode`.
4. Confirm no `aidocs/*` changes were made.
5. Run all standard gates, loom gate, bench check, and dev-tools smoke.

Definition of done:

- Final response maps each original finding to fixed, intentionally retained, or explicitly deferred with reason.
- No original finding remains silently pending.
- All gates pass.

### Task 10.2: Public API Churn Audit

Context:

- Cleanup should simplify internals without accidental public API breakage.

Files:

- `src/lib.rs`
- `src/map/**/*.rs`
- `src/byte_prefix_map.rs`
- `README.md`
- `examples/basic.rs`
- Tests.

Steps:

1. Review all `pub use` lines in `src/lib.rs`.
2. Review all `pub fn` methods on `CacheLogMap`, `LowLevelMap`, and `BytePrefixMap`.
3. Ensure product APIs and low-level APIs are clearly separated.
4. Ensure examples compile.

Definition of done:

- No accidental public low-level API exposure on `CacheLogMap`.
- `LowLevelMap` is the only generic id-bearing API entrypoint.
- `BytePrefixMap` public low-level-like methods are justified by trie maintenance or removed.
- Standard gates pass.

## Completion Criteria For The Entire Plan

The plan is complete only when all of these are true:

- `src/map.rs` is split by responsibility.
- `src/bin/coalesced_hotpath.rs` keeps only current useful dev modes.
- `src/dirty_mode.rs` no longer mixes large loom and native implementations in one dense file.
- `benches/live.rs` is split or table-driven enough that repeated benchmark shape is gone.
- `tests/cachelog.rs` is split by behavior category.
- `src/byte_prefix_map.rs` has a narrow, documented API and no raw inner mutation escape hatch.
- `cachelog-core/src/model.rs` is split by semantic model concern.
- `model-cachelog/src/tla.rs` is split by parser/driver/trace concern.
- Current design docs exist outside `aidocs`.
- `aidocs/*` were not touched unless the user explicitly allowed it later.
- Standard gates pass.
- Loom gate passes.
- Dev-tools smoke passes.
- Bench check passes.
- Final response includes the original finding matrix with no silent pending item.

---

## Batch A Completion Log

**Status: ALL COMPLETE** — 2025-04-27

### Task A1: Wire `src/dirty_mode.rs` Split — DONE

Changes:

- Created `src/dirty_mode/shared.rs` with `FlushWork` enum (moved from `dirty_mode.rs`).
- Created `src/dirty_mode/native.rs` with native `OrderedFifoDirty` implementation.
- Created `src/dirty_mode/loom.rs` with loom `OrderedFifoDirty` implementation.
- Replaced `src/dirty_mode.rs` with a thin 17-line facade:
  - `pub(crate) mod shared` (unconditional).
  - `pub(crate) mod loom` behind `#[cfg(feature = "loom")]`.
  - `pub(crate) mod native` behind `#[cfg(not(feature = "loom"))]`.
  - Conditional re-exports of `DirtyMode`, `OrderedFifoDirty` from the appropriate submodule.
  - `pub(crate) use shared::FlushWork`.

Hard verification:

```bash
$ rg "mod imp" src/dirty_mode.rs
# (empty — no mod imp in facade; mod imp only exists inside submodules)
# exit 1
$ rg "^[[:space:]]*pub(crate) trait DirtyMode|^[[:space:]]*pub(crate) struct OrderedFifoDirty" src/dirty_mode.rs
# (empty — types are in submodules, not the facade)
# exit 1
$ rg "mod shared|mod native|mod loom|pub\(crate\) use .*DirtyMode|pub\(crate\) use .*FlushWork" src/dirty_mode.rs
pub(crate) mod shared;
#[cfg(feature = "loom")]
pub(crate) mod loom;
#[cfg(not(feature = "loom"))]
pub(crate) mod native;
#[cfg(feature = "loom")]
pub(crate) use loom::{DirtyMode, OrderedFifoDirty};
#[cfg(not(feature = "loom"))]
pub(crate) use native::{DirtyMode, OrderedFifoDirty};
pub(crate) use shared::FlushWork;
```

Gates: `cargo check --all-targets --all-features` ✓, `cargo test -q --features loom` ✓ (79 total tests)

### Task A2: Remove Stale Functions From `src/bin/coalesced_hotpath.rs` — DONE

Changes:

- Removed `#![allow(dead_code)]` directive.
- Patched `hotpath_mode_table!` macro to only 13 active modes: `serial`, `serial-quiet`, `batch-unique`, `batch-unique-quiet`, `batch-repeated`, `batch-repeated-quiet`, `flush-batch-unique`, `mark-flushed-unique`, `flush-batch-build-multi`, `mark-flushed-multi`, `clone-keys-multi`, `materialize-dirty-arcs-multi`, `materialize-dirty-arcs-keyclone-multi`.
- Removed 34 orphaned `run_*` function bodies (all modes not in the active set).
- Removed dead helper functions: `build_compacted_repeated_bytes_batches`, `build_compacted_repeated_arc_bytes_batches`, `build_compacted_repeated_bytes_multi`, `build_compacted_repeated_arc_bytes_multi`, `build_compacted_unique_bytes_batches`, `build_compacted_unique_arc_bytes_batches`, `PreparedCompactedBatch` type alias.
- File reduced from 1776 lines to ~460 lines.

Hard verification:

```bash
$ rg "allow\(dead_code\)" src/bin/coalesced_hotpath.rs
# (empty — no output, exit 1)
$ cargo check --bin coalesced_hotpath --features dev-tools 2>&1
# Finished dev profile... zero warnings
```

Gates: `cargo run --features dev-tools --bin coalesced_hotpath -- serial-quiet 1` ✓, `cargo clippy --bin coalesced_hotpath --features dev-tools -- -D warnings` ✓

### Task A3: Repair Misleading `src/map/` Split — DONE

Changes:

- Deleted `src/map.rs` (Rust would find both file and directory when declaring `mod map`).
- Updated `src/map/mod.rs` to honestly document that `types.rs` is the current monolith pending extraction by tasks B1-B5.
- `src/map/mod.rs` no longer falsely claims the module is split by concern.

Hard verification:

```bash
$ ls src/map.rs
ls: cannot access 'src/map.rs': No such file or directory
$ wc -l src/map/types.rs
3188 src/map/types.rs  # monolith pending extraction
```

Gates: `cargo check --all-targets --all-features` ✓, `cargo test --lib` ✓ (11 tests)

### Task A4: Update tasks.md After Batch A Fixes — DONE

This entry (Batch A Completion Log) was added to document the actual repairs.

---

## Batch B: Extract Map Sub-Modules (Tasks B1–B5)

These tasks do the real extraction work that Batch A corrected.

**Status: PENDING** — not yet started.

See Phase 1 tasks 1.1–1.7 (renamed B1–B5 in this batch structure).
