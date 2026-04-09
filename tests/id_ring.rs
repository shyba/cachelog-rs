#![cfg(not(feature = "loom"))]

use cachelog::IdRingMap;

#[test]
fn upsert_then_read_returns_latest() {
    let map = IdRingMap::new(128, 256);

    let first = map.upsert(7, 11);
    let second = map.upsert(7, 22);

    assert!(second > first);
    assert_eq!(map.read(7), Some(22));
}

#[test]
fn flush_unpublishes_matching_id() {
    let map = IdRingMap::new(128, 256);

    map.upsert(3, 30);
    assert_eq!(map.read(3), Some(30));

    let advanced = map.flush_advance(1);
    assert_eq!(advanced, 1);
    assert_eq!(map.read(3), None);
}

#[test]
fn flushing_older_id_does_not_unpublish_newer_same_key() {
    let map = IdRingMap::new(128, 256);

    let old = map.upsert(9, 90);
    let new = map.upsert(9, 99);
    assert!(new > old);

    // Flushing one step targets the oldest id first.
    let advanced = map.flush_advance(1);
    assert_eq!(advanced, 1);

    // Conditional unpublish keeps the newer id visible.
    assert_eq!(map.read(9), Some(99));
}

#[test]
fn flush_advance_zero_is_noop() {
    let map = IdRingMap::new(128, 256);

    map.upsert(1, 10);
    assert_eq!(map.flush_advance(0), 0);
    assert_eq!(map.read(1), Some(10));
}
