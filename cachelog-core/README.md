# cachelog-core

`cachelog-core` is the proof-first semantic core for the cachelog system.

It contains:

- the abstract visible-map state machine
- invariant checks
- the proof-oriented wrappers used by Creusot

It intentionally does **not** contain:

- JSON CLI tooling
- `tla-connect` adapters
- live concurrent data structures

Those stay in sibling crates:

- [`cachelog-model`](/home/user/repos/tableflip-rs/model-cachelog) for CLI, replay, and trace integration
- [`cachelog-rs`](/home/user/repos/tableflip-rs) for the live concurrent implementation

This split exists so the final implementation can follow a tractable proof target
without forcing serialization and tooling concerns into the proof surface.

## Proof Workflow

`cachelog-core` is intentionally a standalone Cargo workspace so Creusot can be
run from this directory without dragging the live crate into proof builds.

Run proof translation from here:

```bash
cd /home/user/repos/tableflip-rs/cachelog-core
cargo creusot --features creusot
```

Run prover integration from here:

```bash
cd /home/user/repos/tableflip-rs/cachelog-core
cargo creusot prove -- --features creusot
```

This crate carries its own [why3find.json](/home/user/repos/tableflip-rs/cachelog-core/why3find.json).
Proof artifacts (`verif/`, `.why3find/`, `target/`) are generated on disk by `cargo creusot prove` but are not tracked in version control.

The sibling crates remain outside the proof root on purpose:

- [`cachelog-model`](/home/user/repos/tableflip-rs/model-cachelog) depends on `cachelog-core`
  for the semantic state machine and adds JSON/TLA adapters.
- [`cachelog-rs`](/home/user/repos/tableflip-rs) is validated against the core model by tests
  and model alignment, not by direct Creusot verification.
