use cachelog::{CacheLogConfig, CacheLogMap};

fn main() {
    let map = CacheLogMap::new(CacheLogConfig::new(16, 16, 16));

    map.put("dirty".to_owned(), 10);

    assert_eq!(map.insert_clean_if_absent("clean".to_owned(), 20), Some(0));

    let dirty = map.read(&"dirty".to_owned(), |key, value, state| {
        (key.clone(), *value, state)
    });
    let clean = map.read(&"clean".to_owned(), |key, value, state| {
        (key.clone(), *value, state)
    });

    println!("dirty read: {dirty:?}");
    println!("clean read: {clean:?}");

    let marked = map
        .with_flush_batch(8, |batch| {
            for entry in batch.iter() {
                println!(
                    "flush candidate: key={:?} value={:?}",
                    entry.key(),
                    entry.value()
                );
            }
            Ok::<_, ()>(())
        })
        .expect("flush batch");
    println!("marked flushed: {marked}");

    assert_eq!(map.read(&"dirty".to_owned(), |_, _, _| ()), None);

    map.put("dirty".to_owned(), 11);
    assert_eq!(
        map.read(&"dirty".to_owned(), |_, value, _| *value),
        Some(11)
    );
}
