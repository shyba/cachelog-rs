use cachelog::{CacheLogConfig, CacheLogMap, VisibleRef};

fn main() {
    let map = CacheLogMap::new(CacheLogConfig::new(16, 16, 16));

    let write_id = map.insert_dirty("dirty".to_owned(), 10);
    assert!(write_id != 0);

    assert_eq!(map.insert_clean_if_absent("clean".to_owned(), 20), Some(0));

    let dirty = map.read(&"dirty".to_owned(), |key, value, state, visible| {
        (key.clone(), *value, state, visible)
    });
    let clean = map.read(&"clean".to_owned(), |key, value, state, visible| {
        (key.clone(), *value, state, visible)
    });

    println!("dirty read: {dirty:?}");
    println!("clean read: {clean:?}");

    let batch = map.flush_batch(8);
    for entry in batch.iter() {
        println!(
            "flush candidate: key={:?} value={:?}",
            entry.key, entry.value
        );
    }

    let marked = map.mark_flushed(&batch);
    println!("marked flushed: {marked}");
    drop(batch);

    assert_eq!(map.read(&"dirty".to_owned(), |_, _, _, _| ()), None);
    assert_eq!(map.visible_ref(&"dirty".to_owned()), None);

    map.insert_dirty("dirty".to_owned(), 11);
    assert!(matches!(
        map.visible_ref(&"dirty".to_owned()),
        Some(VisibleRef::Dirty(_))
    ));
}
