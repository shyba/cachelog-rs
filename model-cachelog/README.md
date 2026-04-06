# cachelog-model

`cachelog-model` is the trace/CLI adapter around the proof-first
[`cachelog-core`](/home/user/repos/tableflip-rs/cachelog-core) state machine,
which in turn is a Rust port of the PlusCal/TLA+ visible-map model in
[../models/cachelog_visible_refs_concurrent](../models/cachelog_visible_refs_concurrent).

It is intentionally separate from the live `cachelog-rs` implementation. This crate
contains:

- re-exports of the deterministic state machine from `cachelog-core`
- executable invariant checks
- a small JSON CLI for state/trace replay
- optional `tla-connect` integration
- no proof logic of its own; proofs live in `cachelog-core`

The live crate currently uses a stronger dirty-visibility implementation detail than the
abstract model: visible dirty entries are published directly via `sdd` and compared by
opaque record identity during flush-side cleanup. The model still matches the semantics
that matter for verification:

- one visible entry per key
- ordered dirty flush
- conditional clearing only when the flushed record is still the visible one
- stale clean windows may exist until cleanup

The model stays slightly more permissive than the current live crate on purpose:

- it keeps separate `FlusherFlushNext` and `FlusherDrop` actions
- it keeps split `CacheDropRecord` and `CacheCleanupVisible` actions

That lets TLC and `tla-connect` explore stale-reference windows even though the
current implementation eagerly removes flushed dirty refs and immediate
`evict_clean()` calls remove visible clean refs directly.

Important boundary: this crate models and replays the abstract state machine. It
does not by itself prove the `scc`/`sdd`/`SegQueue` live implementation in
`cachelog-rs`.

## Default build

The default build does **not** require Apalache, `tla-connect`, Creusot, or Why3.

```bash
cargo test -p cachelog-model
```

## `tla-connect`

`tla-connect` currently requires a newer Rust toolchain than the repo default. In this
environment, use `cargo +nightly` for the feature-gated integration:

```bash
cargo +nightly test -p cachelog-model --features tla-connect
```

The real TraceSpec for Approach 3 lives at:

- [formal/CacheLogVisibleRefsTrace.tla](/home/user/repos/tableflip-rs/model-cachelog/formal/CacheLogVisibleRefsTrace.tla)

Once `apalache-mc` is installed, run the ignored validation test or the helper script:

```bash
cargo +nightly test -p cachelog-model --features tla-connect trace_validation_roundtrip -- --ignored
./model-cachelog/scripts/run-trace-validation.sh
```

To syntax-check and smoke-check the TraceSpec locally with TLC, without Apalache:

```bash
./model-cachelog/scripts/run-tlc-tracespec.sh
```

## Creusot / Why3

Creusot hooks are feature-gated and proof-oriented. They are not exercised by the
default test suite.

## CLI

The crate provides a small CLI:

```bash
cargo run -p cachelog-model -- emit-initial-state
cargo run -p cachelog-model -- apply-step <state.json> <step.json>
cargo run -p cachelog-model -- replay-trace <trace.json>
cargo run -p cachelog-model -- check-state <state.json>
```

There are small ready-to-use fixtures under [fixtures/](/home/user/repos/tableflip-rs/model-cachelog/fixtures):

```bash
cargo run -p cachelog-model -- emit-initial-state > /tmp/state.json
cargo run -p cachelog-model -- apply-step /tmp/state.json model-cachelog/fixtures/step_writer_write_k0_v1.json
cargo run -p cachelog-model -- replay-trace model-cachelog/fixtures/trace_write_flush_drop.json
```
