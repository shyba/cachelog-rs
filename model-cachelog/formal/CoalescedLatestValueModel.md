# Coalesced Latest-Value Model Placeholder

## Goal

Reserve an explicit formal/model landing zone for
`DirtyWriteMode::CoalescedMap`.

This file is a placeholder, not an executable TLA+ spec and not a proved Rust
model. Its job is to make the intended model boundary explicit and prevent the
strict ordered model from being treated as if it already covers coalesced
semantics.

## Model Intent

The coalesced mode should eventually be modeled as a latest-value overlay with
snapshot persistence.

That means the model state is centered on:

- `active`
  - latest writable dirty values
- `draining`
  - stable snapshot currently being persisted
- `clean`
  - optional clean-cache values
- `durable`
  - last successfully persisted values
- `flush_status`
  - idle / draining / failed / shutdown

Unlike the strict model:

- no public `WriteId` is part of the product contract
- no ordered replay of every intermediate write is required
- repeated writes to the same key may collapse before persistence

## Source Notes

This placeholder is derived from the current repository notes:

- [`aidocs/014_coalesced_state_machine_draft.md`](/home/user/repos/tableflip-rs/aidocs/014_coalesced_state_machine_draft.md)
- [`aidocs/015_coalesced_invariant_split.md`](/home/user/repos/tableflip-rs/aidocs/015_coalesced_invariant_split.md)
- [`formal/PersistBatchBoundary.md`](/home/user/repos/tableflip-rs/model-cachelog/formal/PersistBatchBoundary.md)

Those notes remain the authoritative semantic draft until a real model lands.

## Expected Actions

Any future coalesced formalization should cover at least these actions:

1. publish value
2. create flush snapshot
3. persist snapshot
4. retire snapshot
5. fail flush
6. crash

## Expected Invariants

Any future coalesced formalization should prove or check at least:

- latest-value visibility
- dirty beats clean
- snapshot stability during persistence
- durable state equals the last successful snapshot
- flush failure does not silently discard dirty-visible data
- successful retirement removes only the flushed snapshot, not newer writes

## Common API Mapping

The eventual coalesced model must explain the common product API in these
terms:

- `put` / `put_batch`
  - publish into latest-value dirty state
- `read` / `get_cloned`
  - observe visible value and `EntryState`
- `with_flush_batch` / `flush_now`
  - persist a stable key/value snapshot through `PersistBatch`
- `with_persisted_scan`
  - force snapshot persistence before scanning durable state

## Not Yet Represented Here

This placeholder does not yet include:

- a TLA+ module
- a Rust replay model
- trace validation
- proof code in `cachelog-core`

## Exit Condition For Replacing This Placeholder

Replace this note with a real coalesced model only when at least one of these
is needed:

- executable model checking of coalesced semantics
- proof-oriented Rust model support for latest-value persistence
- direct verification of background flush or persisted-scan behavior in
  coalesced terms

Until then, this file exists to keep the repo honest about what is and is not
formally modeled.
