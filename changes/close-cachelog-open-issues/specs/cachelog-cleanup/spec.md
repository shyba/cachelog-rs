# Capability: CacheLog Cleanup

This spec covers only the remaining cleanup issues after the dirty-mode split,
hotpath cull, and initial map split work. It intentionally excludes already
completed repairs such as wiring `src/dirty_mode.rs` and removing stale
`coalesced_hotpath` modes.

## ADDED Requirements

### Requirement: Cleanup handoff state is current
The system SHALL maintain a cleanup handoff that matches the actual working tree and does not claim missing, completed, or orphaned files incorrectly.

#### Scenario: Handoff reflects current map split state
- GIVEN `src/map.rs`, `src/map/types.rs`, `src/map/strict.rs`, `src/map/coalesced.rs`, `src/map/low_level.rs`, and `src/map/api.rs` exist
- WHEN a fresh agent reads the cleanup handoff
- THEN the handoff MUST describe those files as current state rather than saying they do not exist
- AND the handoff MUST identify only the remaining map split work

#### Scenario: Handoff has no false completion claims
- GIVEN a task is marked complete in the handoff
- WHEN its stated verification commands are run
- THEN those commands MUST support the completion claim
- AND any incomplete task MUST remain explicitly pending or partial

### Requirement: Map low-level API is not duplicated
The system SHALL expose generic id-bearing map operations through `LowLevelMap` only once, from the low-level module.

#### Scenario: Low-level wrappers live in one module
- GIVEN the codebase is searched for `impl<.*LowLevelMap`
- WHEN the search scans `src/map.rs` and `src/map/*.rs`
- THEN the public `LowLevelMap` wrapper implementations MUST live in `src/map/low_level.rs`
- AND duplicate `LowLevelMap` implementations MUST NOT remain in `src/map.rs`

#### Scenario: Product map API remains separate from low-level API
- GIVEN the public methods on `CacheLogMap` are reviewed
- WHEN generic dirty id operations are checked
- THEN `CacheLogMap` MUST NOT directly expose public `insert_dirty`, `flush_batch`, `mark_flushed`, `read_full`, or `get_cloned_full`
- AND callers needing those operations MUST use `map.low_level()`

### Requirement: Map debug and conformance helpers are isolated
The system SHALL keep debug snapshots and model conformance helpers outside the product API and core runtime implementation.

#### Scenario: Debug snapshot code is cfg-isolated
- GIVEN `DebugRecord`, `DebugVisible`, `DebugSnapshot`, and `debug_snapshot`
- WHEN the non-loom production build is inspected
- THEN those symbols MUST NOT be exported in the product API
- AND their implementations MUST be behind `#[cfg(any(test, feature = "loom"))]` or a stricter cfg

#### Scenario: Conformance tests do not live in core runtime code
- GIVEN model conformance tests are present
- WHEN the map runtime modules are reviewed
- THEN conformance tests MUST be in a test-only module or file
- AND they MUST NOT increase production map module complexity

### Requirement: Map module boundaries are truthful
The system SHALL split map modules by responsibility and MUST keep module comments accurate to the implemented layout.

#### Scenario: Map files have single clear responsibilities
- GIVEN `src/map.rs` and `src/map/*.rs`
- WHEN line counts and module contents are reviewed
- THEN each map module MUST have an identifiable responsibility such as types, strict engine, coalesced engine, low-level API, product API, debug, or tests
- AND no file MUST be described as "types" while containing broad engine or API implementation bodies

#### Scenario: Map split verification passes
- GIVEN the map split is claimed complete
- WHEN `wc -l src/map.rs src/map/*.rs` and `rg -n "impl<.*LowLevelMap|conformance_tests|pub\\(crate\\) struct StrictEngine|pub\\(crate\\) struct CoalescedEngine" src/map.rs src/map/*.rs` are run
- THEN the output MUST show implementation ownership in the expected files
- AND no orphan module MUST remain unused by the compiled crate

### Requirement: Dirty queue semantics are documented
The system SHALL document strict dirty queue write backpressure and capacity semantics near the configuration and queue implementation.

#### Scenario: Capacity contract is explicit
- GIVEN `CacheLogConfig::dirty_log_capacity` and the strict queue implementation
- WHEN a maintainer reads the docs/comments
- THEN it MUST be clear whether the capacity is a preallocation hint, memory bound, retention limit, or backpressure limit
- AND the documentation MUST match the actual non-blocking write behavior

### Requirement: Dirty borrowed batch duplication is reduced without changing semantics
The system SHALL reduce repeated borrowed-batch construction logic only when strict ordering and write id allocation semantics remain intact.

#### Scenario: Strict append ordering remains serialized
- GIVEN single strict writes and borrowed batch strict writes run concurrently
- WHEN dirty records are flushed
- THEN write ids MUST remain globally unique
- AND `mark_flushed` MUST NOT treat older unflushed records as already flushed because a later writer advanced the frontier first

#### Scenario: Borrowed batch helpers share construction safely
- GIVEN borrowed batch helper code exists for `Vec<u8>`, `Arc<Vec<u8>>`, `Arc<[u8]>`, and `Bytes`
- WHEN duplication is reduced
- THEN the common helper MUST preserve the same values, ids, and queue publication order as the current passing implementation

### Requirement: Hotpath dev binary keeps only current diagnostics
The system SHALL keep the `coalesced_hotpath` dev binary limited to current benchmark modes and helper bodies required by those modes.

#### Scenario: Usage examples are executable
- GIVEN each entry in `HOTPATH_EXAMPLES`
- WHEN the example mode is passed to `coalesced_hotpath`
- THEN the mode MUST be accepted by dispatch rather than falling through to usage

#### Scenario: No hidden dead experiment code remains
- GIVEN `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- WHEN the dev binary is compiled
- THEN it MUST pass without `#![allow(dead_code)]`
- AND stale removed mode functions MUST NOT remain only to be suppressed

### Requirement: Benchmarks are organized by family
The system SHALL organize Criterion benchmarks by family or table-driven sections so current performance coverage remains discoverable.

#### Scenario: Benchmark families are navigable
- GIVEN benchmark coverage for dirty serial writes, dirty batch writes, reads, prefix/list, background flush, and value representation
- WHEN a maintainer opens `benches/`
- THEN each family MUST be identifiable by file name or table-driven section
- AND `benches/live.rs` MUST NOT remain a single large catch-all for unrelated workloads

#### Scenario: Critical performance coverage remains
- GIVEN strict-vs-coalesced serial writes, strict-vs-coalesced batch writes, reads, prefix/list, background flush, and payload/value representation coverage exist before the split
- WHEN the benchmark suite is reorganized
- THEN those categories MUST remain covered
- AND `cargo check --benches --features dev-tools` MUST pass

### Requirement: Integration tests are split by behavior
The system SHALL organize integration tests by behavior category rather than keeping all behavior coverage in one mega-file.

#### Scenario: Test files map to behavior
- GIVEN tests for strict mode, coalesced mode, prefix behavior, background flush, borrowed value paths, concurrency, and product API
- WHEN a maintainer opens `tests/`
- THEN those categories MUST be separated into files or modules with clear names
- AND shared helpers MUST be explicit rather than hidden by broad imports

#### Scenario: Test coverage is preserved
- GIVEN tests are moved or renamed
- WHEN `cargo test -q` and `cargo test -q --all-features` are run
- THEN the moved tests MUST still run
- AND assertions MUST NOT be weakened or deleted to complete the split

### Requirement: BytePrefixMap API is narrow and documented
The system SHALL expose only `BytePrefixMap` product methods and trie-preserving low-level wrappers whose side effects are documented.

#### Scenario: Public BytePrefixMap methods are classified
- GIVEN every public method on `BytePrefixMap`
- WHEN the API is audited
- THEN each method MUST be classified as product API, trie-preserving low-level wrapper, debug/test-only, or removable mirror
- AND removable mirrors MUST be removed or hidden

#### Scenario: Prefix visibility semantics are explicit
- GIVEN point reads and prefix reads can observe data through different mechanisms
- WHEN a caller reads `BytePrefixMap` docs
- THEN the docs MUST state when dirty writes become visible to point reads
- AND the docs MUST state when dirty writes become visible to prefix/list reads
- AND the docs MUST state how background flush affects prefix trie state

### Requirement: Semantic model files are split by concern
The system SHALL split the `cachelog-core` semantic model into modules for state, operations, visibility, invariants, and comparable or serde representations.

#### Scenario: Invariants are easy to locate
- GIVEN a maintainer needs to audit the semantic invariants
- WHEN they inspect `cachelog-core/src/model.rs` and its submodules
- THEN invariant checks MUST live in an obvious module or section
- AND each invariant MUST have a short product-rule comment

#### Scenario: Model public API is preserved
- GIVEN downstream model users compile before the split
- WHEN the model files are split
- THEN the public API and serde behavior MUST remain compatible
- AND the root standard gates MUST pass

### Requirement: TLA bridge parser is separated from driver logic
The system SHALL separate `model-cachelog` TLA value parsing, trace state, and driver or emitter code.

#### Scenario: Parser tests cover moved parser logic
- GIVEN TLA value parsing code is moved
- WHEN focused parser tests run
- THEN atom, string, number, list, map, and malformed-value cases currently supported by the parser MUST be covered

#### Scenario: TLA output format is preserved
- GIVEN existing TLA trace or emitter outputs
- WHEN the bridge is split
- THEN generated formats MUST remain unchanged unless an intentional format migration is documented

### Requirement: Current design docs exist outside historical notes
The system SHALL provide current design documentation outside `aidocs`.

#### Scenario: Fresh agent can find current architecture
- GIVEN a fresh agent reads `README.md`, `DESIGN.md`, and this OpenSpec change
- WHEN it needs to understand the current architecture
- THEN it MUST find the clean/dirty overlay model, strict semantics, coalesced semantics, low-level id boundary, background flush model, and BytePrefixMap trie lifecycle

#### Scenario: Historical notes remain untouched
- GIVEN `aidocs/*` contains historical planning notes
- WHEN current design docs are added or updated
- THEN `aidocs/*` MUST NOT be modified unless the user explicitly authorizes it

### Requirement: Final verification proves no open issue remains
The system SHALL finish with a fresh verification pass that maps each open issue to complete, explicitly deferred, or blocked.

#### Scenario: Final gates are fresh
- GIVEN all cleanup code and docs have stopped changing
- WHEN final verification runs
- THEN the following commands MUST pass after the last relevant mutation:
  - `cargo fmt --check`
  - `cargo check --all-targets --all-features`
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  - `cargo test -q`
  - `cargo test -q --all-features`
  - `cargo test -q --features loom`
  - `cargo test --doc`
  - `cargo check --benches --features dev-tools`
  - `cargo run --features dev-tools --bin coalesced_hotpath -- serial-quiet 1`

#### Scenario: Open issue matrix is complete
- GIVEN the final response claims the cleanup is complete
- WHEN it is compared to this spec
- THEN every Requirement MUST be marked complete with evidence
- AND any deferred work MUST be named with a reason
- AND no unresolved failure, stale evidence, or unverified Requirement MUST be hidden
