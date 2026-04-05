#![cfg(feature = "loom")]

use loom::sync::{Arc, Mutex};
use loom::thread;

#[derive(Debug)]
struct DirtyRecord {
    id: u64,
}

#[derive(Debug)]
struct CleanRecord {
    id: u64,
}

#[derive(Debug, Default)]
struct DirtySlot {
    current: Mutex<Option<Arc<DirtyRecord>>>,
}

impl DirtySlot {
    fn new(record: Arc<DirtyRecord>) -> Self {
        Self {
            current: Mutex::new(Some(record)),
        }
    }

    fn store(&self, record: Arc<DirtyRecord>) {
        *self.current.lock().expect("dirty slot poisoned") = Some(record);
    }

    fn clear_if_matches(&self, expected: &Arc<DirtyRecord>) {
        let mut guard = self.current.lock().expect("dirty slot poisoned");
        if guard
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, expected))
        {
            *guard = None;
        }
    }

    fn current_id(&self) -> Option<u64> {
        self.current
            .lock()
            .expect("dirty slot poisoned")
            .as_ref()
            .map(|record| record.id)
    }
}

#[derive(Debug, Default)]
struct CleanSlot {
    current: Mutex<Option<Arc<CleanRecord>>>,
}

impl CleanSlot {
    fn new(record: Arc<CleanRecord>) -> Self {
        Self {
            current: Mutex::new(Some(record)),
        }
    }

    fn store(&self, record: Arc<CleanRecord>) {
        *self.current.lock().expect("clean slot poisoned") = Some(record);
    }

    fn clear_if_matches(&self, expected_id: u64) {
        let mut guard = self.current.lock().expect("clean slot poisoned");
        if guard
            .as_ref()
            .is_some_and(|current| current.id == expected_id)
        {
            *guard = None;
        }
    }

    fn current_id(&self) -> Option<u64> {
        self.current
            .lock()
            .expect("clean slot poisoned")
            .as_ref()
            .map(|record| record.id)
    }
}

#[test]
fn newer_dirty_visible_survives_old_flush_cleanup() {
    loom::model(|| {
        let old = Arc::new(DirtyRecord { id: 0 });
        let new = Arc::new(DirtyRecord { id: 1 });
        let slot = Arc::new(DirtySlot::new(old.clone()));

        let writer_slot = slot.clone();
        let writer_new = new.clone();
        let writer = thread::spawn(move || {
            writer_slot.store(writer_new);
        });

        let flusher_slot = slot.clone();
        let flusher_old = old.clone();
        let flusher = thread::spawn(move || {
            flusher_slot.clear_if_matches(&flusher_old);
        });

        writer.join().unwrap();
        flusher.join().unwrap();

        assert_eq!(slot.current_id(), Some(1));
    });
}

#[test]
fn current_dirty_is_cleared_when_flush_matches_visible_record() {
    loom::model(|| {
        let current = Arc::new(DirtyRecord { id: 0 });
        let slot = Arc::new(DirtySlot::new(current.clone()));

        let flusher_slot = slot.clone();
        let flusher_current = current.clone();
        let flusher = thread::spawn(move || {
            flusher_slot.clear_if_matches(&flusher_current);
        });

        flusher.join().unwrap();

        assert_eq!(slot.current_id(), None);
    });
}

#[test]
fn newer_clean_visible_survives_fifo_eviction_of_old_clean() {
    loom::model(|| {
        let old = Arc::new(CleanRecord { id: 0 });
        let new = Arc::new(CleanRecord { id: 1 });
        let slot = Arc::new(CleanSlot::new(old.clone()));

        let insert_slot = slot.clone();
        let insert_new = new.clone();
        let inserter = thread::spawn(move || {
            insert_slot.store(insert_new);
        });

        let evict_slot = slot.clone();
        let evictor = thread::spawn(move || {
            evict_slot.clear_if_matches(0);
        });

        inserter.join().unwrap();
        evictor.join().unwrap();

        assert_eq!(slot.current_id(), Some(1));
    });
}
