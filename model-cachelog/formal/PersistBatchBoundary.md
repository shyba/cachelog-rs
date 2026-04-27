# PersistBatch Boundary

## Goal

Define how the root/common `cachelog-rs` `PersistBatch` surface maps onto the
current formal/model layer.

This note is intentionally narrow. It does not introduce a new formal machine.
It records the translation boundary so future work does not over-claim that the
strict replay model already proves the full common persistence surface.

## Current Status

Today:

- `cachelog-core` and `cachelog-model` formally describe the strict ordered,
  id-bearing state machine
- the root/common crate surface exposes persistence through `PersistBatch`
- `PersistBatch` is not a first-class modeled object in the strict proof layer

That means `PersistBatch` should be read as a product-facing projection of
flushable state.

## What `PersistBatch` Means At The Product Layer

For product-facing callers, `PersistBatch` means:

- a stable key/value batch suitable for persistence callbacks
- no public dependence on `WriteId`
- no requirement that the caller understand dirty-record identity
- success retires the corresponding flushable snapshot
- failure preserves dirty-visible data rather than silently discarding it

This is the right common contract for:

- `with_flush_batch(...)`
- `flush_now(...)`
- `with_persisted_scan(...)`
- background flush callbacks

## How `PersistBatch` Maps To The Strict Model Today

In `DirtyWriteMode::StrictLog`, the current mapping is:

1. the strict model selects an ordered low-level flush slice
2. that slice is represented in the live crate as `FlushBatch`
3. the common layer projects that slice to `PersistBatch`
4. the projection erases dirty ids and exposes only `(key, value)` pairs
5. success still retires the same underlying strict flush slice

So the existing formal/model layer proves the source object being projected
from, but not the projection type itself.

## What Is And Is Not Covered By The Current Model

### Covered Indirectly

The current strict model indirectly supports these `PersistBatch` claims for
strict mode:

- the batch comes from a real ordered flushable slice
- the batch is stable while handed to persistence
- successful retirement corresponds to the flushed strict slice
- durable state after success matches replay of flushed strict records

### Not Covered Directly

The current model does not directly encode:

- a first-class `PersistBatch` object
- the common root API contract independent of strict low-level ids
- coalesced snapshot persistence through the same common batch surface
- a mode-agnostic proof that the persist callback only needs key/value pairs

Those are repo-level documented guarantees plus test coverage today.

## Coalesced Meaning

For `DirtyWriteMode::CoalescedMap`, `PersistBatch` should be interpreted as:

- a stable key/value snapshot of latest visible dirty state
- not an ordered replay batch
- not a public identity-bearing record stream

That is why coalesced needs its own latest-value model vocabulary. The strict
ordered model cannot simply be renamed to cover this.

## When `PersistBatch` Would Become A Formal Object

Promote `PersistBatch` into the formal/model layer only if one of these becomes
necessary:

- a mode-agnostic formal common persistence API
- a coalesced formal machine with explicit snapshot persistence steps
- proofs over persisted-scan/background-flush behavior expressed directly in
  root/common API terms

Until then, the right interpretation is:

- strict model proves the id-bearing source state
- common crate exposes a projected key/value persistence surface
- coalesced semantics are specified by latest-value notes and validated by
  tests

## Immediate Repository Rule

When documenting or reviewing persistence behavior:

- if the claim depends on ordered dirty ids, point to the strict model
- if the claim only depends on stable key/value persistence callbacks, point to
  `PersistBatch` and the common product notes
- if the claim depends on latest-value collapse or snapshot retirement, point
  to coalesced-specific notes rather than the strict replay model
