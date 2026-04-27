# cachelog

A concurrent key-value map with a built-in dirty overlay and cache layer.

Writes become immediately readable through the overlay. In `StrictLog` mode
they also append to an ordered dirty log; in `CoalescedMap` they publish as a
latest-value dirty view. A flusher drains pending dirty state to durable
storage. Once flushed, entries can be cached back as clean reads.

## Quick start

```rust
use cachelog::{CacheLogConfig, CacheLogMap};

// capacity: visible index, dirty log, clean cache
let map = CacheLogMap::new(CacheLogConfig::new(1024, 1024, 1024));

// common write — immediately visible
map.put("key".to_owned(), 42);

// read
let value = map.read(&"key".to_owned(), |_key, value, _state| *value);
assert_eq!(value, Some(42));

// flush to durable storage through the common persist surface
let flushed = map
    .with_flush_batch(64, |batch| {
        for entry in batch.iter() {
            let _key = entry.key();
            let _value = entry.value();
            // ... persist key/value ...
        }
        Ok::<_, ()>(())
    })
    .unwrap();
assert_eq!(flushed, 1);

// after flushing, the dirty entry is removed
assert!(map.read(&"key".to_owned(), |_, _, _| ()).is_none());

// populate clean cache from durable storage
map.insert_clean_if_absent("key".to_owned(), 42);
```

## How it works

The map tracks every entry as either **dirty** (needs flushing) or **clean**
(already persisted, cached for fast reads).

- `put` installs a visible dirty value
- `with_flush_batch` is the common persist helper
- `flush_batch` + `mark_flushed` remain available as the low-level id-bearing flush path
- `insert_clean_if_absent` populates the bounded clean cache
- `read` returns whichever entry is visible, dirty or clean, along with `EntryState`
- Dirty always wins — a dirty write replaces any existing clean entry

The flusher never blocks readers. Old dirty entries are removed only when the
exact flushed record still matches the visible reference, so newer writes are
never lost.


## Write modes

`CacheLogConfig` supports two dirty write modes:

- `DirtyWriteMode::StrictLog` (default): every dirty write is logged and flushed in write order.
- `DirtyWriteMode::CoalescedMap`: batch writes can coalesce repeated keys (last value wins within the batch) for higher producer throughput.

Use `StrictLog` when replaying every intermediate write matters.
Use `CoalescedMap` when latest-value durability is sufficient and throughput is prioritized.

Lower-level APIs such as `insert_dirty` and the id-bearing types under
`cachelog::low_level` still exist for strict-mode internals, tests, and
engine-specific callers, but they are no longer the intended common product
surface.

## Batch writes

- `put_batch` is the common batch write API.
- `insert_dirty_batch` remains available for callers that still need returned write ids.
- In `StrictLog`, all batch entries are appended/flushed in order.
- In `CoalescedMap`, repeated keys within the same batch are deduplicated and published directly into the visible map (no ordered dirty-log append path).


## Dirty queue behavior

Dirty writes use the bounded crossbeam channel path.

`CoalescedMap` bypasses the ordered dirty queue on write-path publication and
flushes from detached latest-value snapshots rather than draining the live
writer map.

## Background flush API

Cachelog exposes a product-side background flusher:

- `CacheLogMap::start_background_flush(config, persist)`
- `CacheLogMap::start_background_flush_default(persist)`
- `CacheLogMap::start_background_flush_for_batch_size(batch_size, persist)`
- `BackgroundFlushConfig::for_batch_size(batch_size)`

The caller provides only the persistence closure. Runtime policy changes stay on the returned
`BackgroundFlushHandle`, including `reconfigure(trigger_dirty, flush_limit)`,
`reconfigure_for_batch_size(batch_size)`, `request_flush()`, `flush_sync()`, and `shutdown()`.

The product-facing persistence contract remains key/value-oriented through
`PersistBatch`. Exact flush-record identity stays on the low-level id-bearing
surface.

`BackgroundFlushConfig::for_batch_size(batch_size)` matches the current ElmDB-side policy shape:
flush after roughly `batch_size` dirty writes and use `flush_limit = 512`.

## Read boundary

Cachelog exposes two distinct read models:

- `with_prefix_snapshot(prefix, limit, |keys| ...)` for overlay-backed prefix/list reads
- `with_persisted_scan(flush_limit, persist, || ...)` for reads that must agree with persisted state

The first reads visible overlay state directly and does not flush. The second
flushes pending dirty data first, then runs the caller's scan.

The common persist callbacks for `with_flush_batch(...)`, `flush_now(...)`,
`with_persisted_scan(...)`, and background flush receive `PersistBatch`
entries with `key()` / `value()` accessors. Low-level `FlushBatch` remains
available under `cachelog::low_level` for id-bearing flush control paths.

`with_prefix_snapshot(...)` returns the lexicographically first `limit` matching visible keys, not
an arbitrary hash-iteration subset.

`BytePrefixMap` now mirrors the same persistence boundary on top of its trie-backed
prefix index:

- `with_flush_batch(...)`
- `flush_now(...)`
- `with_persisted_scan(...)`
- `start_background_flush(...)`
- `start_background_flush_default(...)`
- `start_background_flush_for_batch_size(...)`

## Verification boundary

This repository has a verified **semantic core**, not an end-to-end formal proof
of the live concurrent crate.

- **TLA+ model checking** explores the abstract visible-map state machine and its
  split writer/flusher/cache/reader/crash actions.
  The human-maintained PlusCal/TLA+ source models live under `models/`, while
  the active trace-validation spec used by `model-cachelog` lives under
  `model-cachelog/formal/`
- **Creusot proofs** verify invariant preservation for the deterministic core
  state machine in [`cachelog-core`](/home/user/repos/tableflip-rs/cachelog-core)
- **Conformance tests** compare `StrictLog` live behavior against that core model on
  deterministic operation sequences
- **Loom tests** explore selected interleavings of the live concurrent
  implementation

What is **not** proved today:

- the `scc`-based live implementation in [src/map.rs](/home/user/repos/tableflip-rs/src/map.rs)
- linearizability of the live API against the abstract model
- crash/recovery behavior of the live crate

The live crate still is not end-to-end deductively verified. Ordered dirty ids
remain part of the strict-mode/low-level surface, not the common product
contract.

## Building

```sh
cargo build
cargo test
cargo bench
```
