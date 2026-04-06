# cachelog-rs

A concurrent key-value map with a built-in write log and cache layer.

Writes go into an ordered log and become immediately readable. A background
flusher drains the log to durable storage. Once flushed, entries can be
cached back as clean reads. The map handles all of this behind a single
lookup interface.

## Quick start

```rust
use cachelog_rs::{CacheLogConfig, CacheLogMap};

// capacity: visible index, dirty log, clean cache
let map = CacheLogMap::new(CacheLogConfig::new(1024, 1024, 1024));

// write — immediately visible
map.insert_dirty("key".to_owned(), 42);

// read
let value = map.read(&"key".to_owned(), |_key, value, _state, _ref| *value);
assert_eq!(value, Some(42));

// flush to durable storage
let batch = map.flush_batch(64);
// ... persist batch entries to disk/db ...
map.mark_flushed(&batch);

// after flushing, the dirty entry is removed
assert!(map.read(&"key".to_owned(), |_, _, _, _| ()).is_none());

// populate clean cache from durable storage
map.insert_clean_if_absent("key".to_owned(), 42);
```

## How it works

The map tracks every entry as either **dirty** (needs flushing) or **clean**
(already persisted, cached for fast reads).

- `insert_dirty` appends to the write log and installs a visible reference
- `flush_batch` + `mark_flushed` drain the log in write order
- `insert_clean_if_absent` populates the bounded clean cache
- `read` returns whichever entry is visible, dirty or clean
- Dirty always wins — a dirty write replaces any existing clean entry

The flusher never blocks readers. Old dirty entries are removed only when the
exact flushed record still matches the visible reference, so newer writes are
never lost.

## Verification boundary

This repository has a verified **semantic core**, not an end-to-end formal proof
of the live concurrent crate.

- **TLA+ model checking** explores the abstract visible-map state machine and its
  split writer/flusher/cache/reader/crash actions
  (`models/` and `model-cachelog/formal/`)
- **Creusot proofs** verify invariant preservation for the deterministic core
  state machine in [`cachelog-core`](/home/user/repos/tableflip-rs/cachelog-core)
- **Conformance tests** compare the live crate against that core model on
  deterministic operation sequences
- **Loom tests** explore selected interleavings of the live concurrent
  implementation

What is **not** proved today:

- the `scc`-based live implementation in [src/map.rs](/home/user/repos/tableflip-rs/src/map.rs)
- linearizability of the live API against the abstract model
- crash/recovery behavior of the live crate

The live crate still is not end-to-end deductively verified, but it now shares
the same stable logical dirty-id semantics as the abstract model: live
`VisibleRef::Dirty` values correspond to logical dirty write ids rather than
opaque record tokens.

## Building

```sh
cargo build
cargo test
cargo bench
```
