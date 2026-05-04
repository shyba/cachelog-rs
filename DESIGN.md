# CacheLog Design

## What is CacheLog?

CacheLog is a **concurrent visible-index key-value map** with a built-in write-ahead log. It provides:

- A single concurrent map keyed by `K` with in-memory reads
- A clean cache store for persisted entries
- A dirty overlay for unflushed writes
- A background flush service that drains the dirty overlay into the clean store

It is **not** a database. It does not manage files, sockets, or I/O devices directly. The caller supplies a `PersistBatch` callback (e.g., to write to LMDB, RocksDB, a socket, etc.).

---

## Product Model

### Clean Store

The **clean store** is the persisted layer. Entries installed as `Clean` hold a `CacheId` and a value. Reads against a clean entry return `(value, EntryState::Clean)`. The clean store is the durable, "already-flushed" state of the map.

### Dirty Overlay

The **dirty overlay** is an in-memory write buffer. Dirty entries are indexed by key and overlaid on top of the clean store for reads. Each dirty entry carries a `WriteId` (a monotonically increasing `u64`). Writes to the same key produce a new dirty entry with a new `WriteId`; the overlay retains only the latest per-key value.

A dirty entry is **not** visible to reads until it is explicitly published via the `DirtyMode` queue.

### Point Reads

A **point read** (`get(&K)`) returns `(value, EntryState)` by checking the dirty overlay first (returning the latest dirty value if present), then falling back to the clean store. The caller does **not** need to know `WriteId` or `CacheId` — common reads use `PersistBatch` and `EntryState`, not raw id-bearing records.

### Prefix Reads

A **prefix read** (`with_prefix_snapshot(...)`) walks the `BytePrefixMap` trie to collect all keys sharing a given byte prefix. It returns a point-in-time snapshot of the clean store at the moment of the snapshot, without forcing dirty entries to be flushed. Prefix reads are for scanning/searching use cases where dirty-in-flight writes need not be included.

---

## Strict Mode Semantics (`DirtyWriteMode::StrictLog`)

In **StrictLog** mode, every dirty write is appended to an ordered FIFO queue (`OrderedFifoDirty`). The `WriteId` is the position in that FIFO. Flush extracts a contiguous prefix of the queue (up to `flush_limit`) as a `FlushBatch`.

Key properties:
- **Ordered flush**: flush order respects append order. If write A is appended before write B, A will always be in an earlier flush batch than B.
- **Visible dirty**: `VisibleRef::Dirty(WriteId)` — readers can see which generation of the dirty overlay they are reading.
- **No coalescing**: writing the same key twice produces two dirty entries in the queue. Both will be flushed (the later one overwriting the earlier).
- **Strict backlog ordering**: the dirty backlog is the single source of truth for flush ordering.

This mode is validated against the TLA+ state machine in `cachelog-core` via conformance tests and loom scenarios.

---

## Coalesced Mode Semantics (`DirtyWriteMode::CoalescedMap`)

In **CoalescedMap** mode, dirty writes are **not** appended to an ordered log. Instead, each write publishes directly into a latest-value overlay (a `ConcurrentHashMap<K, DirtyRecord>`). Only one dirty entry per key exists in the overlay at any time — later writes **overwrite** earlier writes to the same key.

Flush extracts a **detached snapshot** of the overlay as a `FlushBatch`. The snapshot is taken under lock, yielding a consistent view of the latest values for a subset of keys.

Key properties:
- **No ordered flush guarantee**: writes to different keys can be flushed in any order. Writes to the same key are always latest-value (the overwrite is guaranteed).
- **Visible dirty**: `VisibleRef::Dirty(WriteId)` still exists, but the `WriteId` is a logical sequence number, not a position in a FIFO.
- **Throughput-optimized**: avoids the per-key write amplification of StrictLog for workloads with many duplicate-key overwrites.
- **Relaxed semantics**: intentionally does not provide the same ordered-flush invariants as StrictLog. It is tested for safety/invariants but not deductively verified against the formal model.

---

## Why WriteIds Are Low-Level Only

`WriteId` (`u64`) and `CacheId` (`u64`) are **raw identifiers**. They are not part of the product API. They are exposed only under `cachelog::low_level`:

```rust
pub mod low_level {
    pub use crate::entry::{CacheId, CleanRecord, DirtyRecord, FlushBatch, VisibleRef, WriteId};
    pub use crate::map::LowLevelMap;
}
```

The product API uses `EntryState` (`Clean`/`Dirty`) and `PersistBatch` — not id-bearing records. This boundary exists because:

1. **Consumers should not depend on id mechanics** — the map can evolve its id scheme without breaking callers.
2. **Prefix reads and point reads do not require ids** — they operate on values and `EntryState`.
3. **Flush ordering is abstracted** — `PersistBatch` represents a batch of records to persist, without forcing callers to understand `WriteId` sequencing.
4. **Formal verification boundary** — `cachelog-core` proves properties of the state machine using these ids, but the product API is decoupled from that proof.

---

## Background Flush Model and Failure Behavior

### BackgroundFlushService

The `BackgroundFlushService` runs a dedicated background thread. It monitors the dirty count and periodically wakes to flush batches. Configuration:

```rust
pub struct BackgroundFlushConfig {
    pub trigger_dirty: usize,   // dirty count threshold to trigger flush
    pub flush_limit: usize,     // max entries per flush batch
    pub check_interval: Duration, // polling interval
}
```

### Flush Lifecycle

1. Background thread wakes (on timer or when `trigger_dirty` is exceeded).
2. Calls `OrderedFifoDirty::flush_batch(limit)` (StrictLog) or takes a snapshot (CoalescedMap).
3. Hands the `FlushBatch` to the `BackgroundFlushHandle`.
4. The handle invokes the caller's `persist_batch` callback.
5. On success, the flushed entries are removed from the dirty overlay.

### Failure Behavior

- **Persist callback failure**: if `persist_batch` returns an error or panics, the `BackgroundFlushHandle` logs the error and **retains** the dirty entries in the overlay. They are **not** dropped or acknowledged. A subsequent flush will retry them.
- **Retry policy**: callers should implement exponential backoff or circuit-breaking in their `persist_batch` callback.
- **Force flush signal**: callers can send `FlushWork::ForceFlush` to bypass the dirty-count trigger and immediately flush.
- **Thread shutdown**: on drop, the `BackgroundFlushService` attempts to flush remaining dirty entries synchronously before stopping.

---

## BytePrefixMap Trie Lifecycle

`BytePrefixMap` is a **trie** (backed by `qp-trie`) that maps `Arc<[u8]>` byte sequences to `CacheLogMap` entries. It is used for **prefix scans** — finding all keys that share a common byte prefix.

Its public methods fall into four buckets:

- Product API mirrors: `new`, `with_hasher`, `visible_len`, `dirty_log_len`,
  `clean_store_len`, `read`, `read_full`, `put`, `put_batch`,
  `insert_dirty`, `upsert_dirty`, `insert_clean_if_absent`.
- Trie-preserving flush API: `flush_batch`, `with_flush_batch`, `flush_now`,
  `with_persisted_scan`, `mark_flushed`.
- Trie publication API: `advance_trie`, `for_each_prefix_key`, `list_prefix`.
- Background flush API: `start_background_flush`,
  `start_background_flush_default`, `start_background_flush_for_batch_size`.

### Lifecycle

1. **Construction**: created empty when `CacheLogMap` is created.
2. **Insertion**: when a clean entry is installed (`EntryState::Clean`), its key bytes are inserted into the trie.
3. **Removal**: when a clean entry is evicted, its key bytes are removed from the trie.
4. **Prefix snapshot**: `list_prefix(prefix, ...)` / `for_each_prefix_key(prefix, ...)` walk the trie under the prefix and return a point-in-time view of the currently published keys.
5. **Seal**: the trie itself is never "sealed" — it is always readable. But note: **dirty entries are not in the trie until `advance_trie` runs**. Prefix scans only see published keys. This is intentional — prefix scans are for cache-friendly scanning without dirty-write interference.

### Thread Safety

`BytePrefixMap` uses `RwLock` for interior mutability. Readers acquire a read lock for prefix iteration; writers acquire a write lock for insert/remove. The trie is `Sync + Send` and can be accessed from any thread.

---

## Testing and Benchmarking Gates

### Test Gates

| Gate | Command | Purpose |
|------|---------|---------|
| Unit tests | `cargo test` | Fast unit/integration tests, no feature flags |
| Doc tests | `cargo test --doc` | Documentation examples, validates doc comments |
| Loom tests | `cargo test --features loom` | Concurrency stress testing via `loom` crate |
| Loom exhaustive | `cargo test --features loom loom_exhaustive` | Exhaustive interleaving exploration |
| Dev-tools perf | `cargo test --features dev-tools` | Performance counter tests |
| Model tests | `cargo test -p model-cachelog` | TLA+ trace validation |
| Core proofs | `cargo test -p cachelog-core` | Creusot/Why3 formal proofs |

### Benchmark Gates

| Gate | Command | Purpose |
|------|---------|---------|
| Live bench | `cargo bench --features dev-tools` | In-process live benchmarks via `criterion` |
| Hotpath bench | `cargo run --features dev-tools --bin coalesced_hotpath` | Single-write coalesced hotpath microbenchmark |

Note: `autobenches = false` in `Cargo.toml` disables the standard `cargo bench` harness for the lib itself. The actual benchmarks live in `benches/live.rs` and as a separate binary.

---

## What `aidocs/` Are

The `aidocs/` directory contains **historical design notes, planning documents, and past experiments** — not the current source of truth.

Examples:
- `001_creusot_inductive_invariants_plan.md` — early plan for formal verification
- `009_coalesced_overlay_redesign_plan.md` — past redesign explorations
- `014_coalesced_state_machine_draft.md` — previous state machine drafts

These files represent decisions that were made, reversed, or evolved over time. They are kept for **archival context** — to understand why the current design is shaped the way it is.

**Do not edit `aidocs/` files.** They are frozen historical records. If something in `aidocs/` conflicts with the current implementation, the implementation wins — `aidocs/` is out of date.

The authoritative current design entry points are:
1. `README.md` — what the crate does and how to use it
2. `DESIGN.md` — this file — architecture, semantics, and invariants
3. `tasks.md` — current work items and cleanup phases
