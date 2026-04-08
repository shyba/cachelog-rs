# CacheLog Visible-Refs Concurrent Model

This directory contains a **PlusCal** concurrent model for the single-visible-map
design:

- one `Visible` map used by reads
- one ordered dirty `WriteLog`
- one optional `CacheStore`
- `DirtyRef` points only into the write log
- `CleanRef` points only into the cache store
- reads do **not** mutate cache state

The live `cachelog-rs` crate currently implements a stricter variant of this
design:

- dirty visibility is removed by the flusher at `mark_flushed()` time if the
  visible entry still matches the flushed record
- explicit `evict_clean()` removes the visible clean entry immediately

The model intentionally keeps the looser split actions below so TLC can still
exercise stale-reference interleavings as a semantic envelope.

Unlike the simpler semantic model, this one has separate PlusCal processes so TLC
explores interleavings between:

- `Writer`
- `Flusher`
- `CacheMaintainer`
- `Reader`
- `Crasher`

## Important abstraction choice

Cache eviction is modeled in two phases:

1. `CacheDropRecord`: remove the cache-owned record
2. `CacheCleanupVisible`: later erase any stale `CleanRef`

This intentionally allows a stale `CleanRef` to exist briefly so the `Reader`
process can exercise the rule:

- stale `CleanRef` must fall back to `Durable`/miss
- reads still do **not** perform cache mutation

## What TLC checks

- state remains well-typed
- visible `DirtyRef`s always point to live dirty records
- dirty queue and flushed sequence stay ordered
- `Durable` is exactly the flushed prefix
- dirty writes are not lost before crash
- reads never produce an invalid result (`NoBadRead`)

`NoBadRead` is tracked by a sticky `badRead` flag set only if the `Reader`
observes an impossible state, such as:

- `DirtyRef` to a dead dirty record
- live `DirtyRef` or live `CleanRef` pointing at a record for the wrong key

## Running TLC

From the repo root:

```bash
tlc -deadlock -config models/cachelog_visible_refs_concurrent/CacheLogVisibleRefsConcurrent.cfg \
  models/cachelog_visible_refs_concurrent/CacheLogVisibleRefsConcurrent.tla
```
