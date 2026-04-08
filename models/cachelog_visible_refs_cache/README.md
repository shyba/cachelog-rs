# CacheLog Visible-Refs Cache Model

This directory contains a **PlusCal** semantic model for the newer single-visible-map
design:

- one `Visible` map for reads
- one ordered dirty `WriteLog`
- one optional `CacheStore`
- `DirtyRef` points only into the write log
- `CleanRef` points only into the cache store
- reads do **not** mutate cache state or emit cache-touch events

The point of the model is to keep cache policy abstract.

## What is modeled

- `Visible[k]` is one of:
  - `NoneRef`
  - `DirtyRef(id)` for a live write-log record
  - `CleanRef(id)` for a live cache record
- `WriteStore` owns live dirty records.
- `WriteHist` remembers all dirty records ever created so flushed-prefix semantics remain
  well-defined even after a dirty record is dropped from `WriteStore`.
- `DirtyQ` is the ordered queue of dirty-flush obligations.
- `Flushed` is the ordered sequence of completed dirty flushes.
- `Durable` is reconstructed by applying `Flushed` in order.
- `CacheStore` owns optional clean cache entries.

## What is deliberately *not* modeled

- no LRU/FIFO/TinyLFU policy
- no touch bookkeeping
- no read-side cache mutation
- no buckets, locks, hashes, or Rust memory model details

Cache behavior is represented only by nondeterministic cache actions:

- `CacheInsert`
- `CacheEvict`

This keeps the model valid whether caching is disabled, FIFO, LRU-like, or any other
policy.

## Main actions

- `WriteDirty(k, v)`: append a dirty record, install `DirtyRef`
- `FlushNext`: flush the next dirty record into `Durable`
- `DropFlushed`: drop a live dirty record after flush and erase any matching `DirtyRef`
- `CacheInsert`: create a fresh `CleanRef` from `Durable` if no dirty ref is visible
- `CacheEvict`: remove a clean cache record and erase matching `CleanRef`s
- `Crash`: clear all volatile state and preserve only `Durable`

## What TLC checks

- `Visible` refs are type-correct
- every `DirtyRef` points to a live write-log record
- every `CleanRef` points to a live cache record
- dirty queue and flushed order stay strictly increasing
- `Durable` always matches the flushed prefix exactly
- no dirty write is lost before crash

## Running TLC

From the repo root:

```bash
tlc -deadlock -config models/cachelog_visible_refs_cache/CacheLogVisibleRefsCache.cfg \
  models/cachelog_visible_refs_cache/CacheLogVisibleRefsCache.tla
```
