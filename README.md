# cachelog

A concurrent key-value map with a built-in dirty overlay and cache layer.

Writes become immediately readable through the overlay. In `StrictLog` mode
they also append to an ordered dirty log; in `CoalescedMap` they publish as a
latest-value dirty view. A flusher drains pending dirty state to durable
storage. Once flushed, entries can be cached back as clean reads.

For prefix scans, `BytePrefixMap` layers a `qp-trie` over the core map. Its
trie visibility is decoupled from dirty visibility: call `advance_trie` to
publish queued keys, then use `list_prefix` or `for_each_prefix_key` to scan
the published prefix set.

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

## Architecture

See [DESIGN.md](DESIGN.md) for architecture details.

## Building

```sh
cargo build
cargo test
cargo bench
```
