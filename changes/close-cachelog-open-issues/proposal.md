# Close CacheLog Cleanup Open Issues

## Why

The CacheLog cleanup is partially complete, but the repository still has open
maintainability issues: duplicated low-level map wrappers, stale cleanup handoff
state, remaining map split work, unsplit benchmark and integration-test mega
files, broad `BytePrefixMap` API surface, dense model/TLA files, and missing
current design documentation.

The goal is to finish the cleanup without regressing strict-log ordering,
coalesced latest-value semantics, loom coverage, or dev-tool benchmark gates.

## What Changes

- Bring the cleanup handoff back in sync with the actual working tree.
- Finish the map module split by responsibility and remove duplicate low-level
  wrappers.
- Isolate debug/conformance helpers away from product runtime code.
- Document strict dirty queue capacity/backpressure semantics.
- Reduce dirty borrowed-batch duplication only when ordering and id semantics
  stay intact.
- Keep `coalesced_hotpath` limited to current useful diagnostics.
- Split or table-drive benchmarks and integration tests by behavior.
- Narrow and document `BytePrefixMap` public surface.
- Split semantic model and TLA bridge files by concern.
- Add current design docs outside `aidocs`.
- Finish with a fresh verification pass and an open-issue matrix.

## Impact

- Internal module structure changes across `src/map*`, `src/dirty_mode*`,
  benches, tests, and model crates.
- Public product APIs should remain compatible unless a method is explicitly
  identified as a removable mirror.
- `cachelog::low_level` remains the explicit boundary for generic id-bearing
  operations.
- Verification remains Rust-first: formatting, check, clippy, tests, loom,
  doc tests, bench check, and dev-tools smoke.

## Non-Goals

- Do not edit `aidocs/*`.
- Do not redesign strict-log or coalesced write semantics.
- Do not change benchmark measured behavior while reorganizing benchmarks.
- Do not remove correctness tests or weaken assertions.
- Do not introduce new dependencies.
