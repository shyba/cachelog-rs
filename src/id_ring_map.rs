use scc::HashMap as ConcurrentHashMap;

use crate::ring::{Direct, MonotonicRing, PAYLOAD_WORDS, Payload};

/// Experimental map mode:
/// - visible map stores `key -> latest write_id`
/// - monotonic ring stores ordered write payloads
///
/// Payload layout:
/// - word 0: key
/// - word 1: value
/// - words 2..: zero
pub struct IdRingMap {
    ids: ConcurrentHashMap<u64, u64>,
    ring: MonotonicRing<Direct>,
}

impl IdRingMap {
    pub fn new(visible_capacity: usize, ring_capacity: usize) -> Self {
        Self {
            ids: ConcurrentHashMap::with_capacity(visible_capacity.max(1)),
            ring: MonotonicRing::new(ring_capacity.max(1)),
        }
    }

    #[inline]
    fn encode(key: u64, value: u64) -> Payload {
        let mut payload = [0u64; PAYLOAD_WORDS];
        payload[0] = key;
        payload[1] = value;
        payload
    }

    #[inline]
    fn decode(payload: &Payload) -> (u64, u64) {
        (payload[0], payload[1])
    }

    /// Inserts or overwrites a key, returning the write id allocated in ring order.
    pub fn upsert(&self, key: u64, value: u64) -> u64 {
        let id = self.ring.push(Self::encode(key, value));
        match self.ids.entry_sync(key) {
            scc::hash_map::Entry::Occupied(mut o) => {
                let _ = o.insert(id);
            }
            scc::hash_map::Entry::Vacant(v) => {
                v.insert_entry(id);
            }
        }
        id
    }

    /// Reads the currently visible value for `key`.
    ///
    /// Returns `None` if:
    /// - key is absent
    /// - the pointed write id was overwritten in ring
    /// - generation/key check fails
    pub fn read(&self, key: u64) -> Option<u64> {
        let id = self.ids.read_sync(&key, |_, id| *id)?;
        self.ring.read(id, |payload| {
            let (stored_key, value) = Self::decode(payload);
            if stored_key == key { Some(value) } else { None }
        })?
    }

    /// Advances flush frontier by up to `n` ids.
    ///
    /// Per id, the sequence is:
    /// 1. resolve `(key, value)` from ring id for flushing work
    /// 2. conditionally unpublish `key` if it still points to this id
    /// 3. drop the ring slot by advancing `base`
    ///
    /// This intentionally pushes work to flusher threads.
    pub fn flush_advance(&self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        let mut advanced = 0u64;
        for _ in 0..n {
            let id = self.ring.base();
            if id >= self.ring.top() {
                break;
            }

            if let Some(key) = self.ring.read(id, |payload| payload[0]) {
                // Unpublish only if this key still points at this flushed id.
                let _ = self.ids.remove_if_sync(&key, |current_id| *current_id == id);
            }

            let step = self.ring.pop_n(1);
            if step == 0 {
                break;
            }
            advanced += step;
        }
        advanced
    }

    #[inline]
    pub fn base(&self) -> u64 {
        self.ring.base()
    }

    #[inline]
    pub fn top(&self) -> u64 {
        self.ring.top()
    }
}
