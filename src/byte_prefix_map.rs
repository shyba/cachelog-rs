use std::borrow::Borrow;
use std::cmp::Ordering;
use std::collections::HashSet;
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::hash::{Hash, Hasher};

use kanal::{Receiver, Sender};
use parking_lot::{Mutex, RwLock};
use qp_trie::Trie;

#[cfg(not(feature = "loom"))]
use crate::sync::Arc;
use crate::{CacheId, CacheLogConfig, CacheLogMap, EntryState, FlushBatch, VisibleRef, WriteId};

#[derive(Clone, Debug)]
struct PrefixKey {
    #[cfg(feature = "loom")]
    inner: Vec<u8>,
    #[cfg(not(feature = "loom"))]
    inner: Arc<[u8]>,
}

impl PrefixKey {
    fn new(key: Vec<u8>) -> Self {
        #[cfg(feature = "loom")]
        {
            Self { inner: key }
        }
        #[cfg(not(feature = "loom"))]
        {
            Self {
                inner: Arc::from(key.into_boxed_slice()),
            }
        }
    }

    fn as_slice(&self) -> &[u8] {
        #[cfg(feature = "loom")]
        {
            self.inner.as_slice()
        }
        #[cfg(not(feature = "loom"))]
        {
            self.inner.as_ref()
        }
    }
}

impl Borrow<[u8]> for PrefixKey {
    fn borrow(&self) -> &[u8] {
        self.as_slice()
    }
}

impl PartialEq for PrefixKey {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl Eq for PrefixKey {}

impl PartialOrd for PrefixKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PrefixKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_slice().cmp(other.as_slice())
    }
}

impl Hash for PrefixKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_slice().hash(state);
    }
}

/// Byte-key cachelog wrapper with a decoupled prefix trie.
///
/// Writers enqueue keys quickly; callers can advance trie state explicitly via
/// [`advance_trie`] from a flusher/maintenance thread when idle.
pub struct BytePrefixMap<V, H = RandomState>
where
    H: BuildHasher + Clone,
{
    inner: CacheLogMap<Vec<u8>, V, H>,
    prefix_trie: RwLock<Trie<PrefixKey, ()>>,
    updates_tx: Sender<PrefixKey>,
    updates_rx: Mutex<Receiver<PrefixKey>>,
}

impl<V> BytePrefixMap<V, RandomState> {
    pub fn new(config: CacheLogConfig) -> Self {
        Self::with_hasher(config, RandomState::new())
    }
}

impl<V, H> BytePrefixMap<V, H>
where
    H: BuildHasher + Clone,
{
    pub fn with_hasher(config: CacheLogConfig, build_hasher: H) -> Self {
        let (updates_tx, updates_rx) = kanal::unbounded();
        Self {
            inner: CacheLogMap::with_hasher(config, build_hasher),
            prefix_trie: RwLock::new(Trie::new()),
            updates_tx,
            updates_rx: Mutex::new(updates_rx),
        }
    }

    pub fn inner(&self) -> &CacheLogMap<Vec<u8>, V, H> {
        &self.inner
    }

    pub fn visible_len(&self) -> usize {
        self.inner.visible_len()
    }

    pub fn dirty_log_len(&self) -> usize {
        self.inner.dirty_log_len()
    }

    pub fn clean_store_len(&self) -> usize {
        self.inner.clean_store_len()
    }

    pub fn read<R>(
        &self,
        key: &[u8],
        reader: impl FnOnce(&Vec<u8>, &V, EntryState, VisibleRef) -> R,
    ) -> Option<R> {
        self.inner.read(key, reader)
    }

    pub fn insert_dirty(&self, key: Vec<u8>, value: V) -> WriteId {
        let prefix_key = PrefixKey::new(key.clone());
        let id = self.inner.insert_dirty(key, value);
        let _ = self.updates_tx.send(prefix_key);
        id
    }

    pub fn upsert_dirty(&self, key: Vec<u8>, value: V) -> WriteId {
        self.insert_dirty(key, value)
    }

    pub fn insert_clean_if_absent(&self, key: Vec<u8>, value: V) -> Option<CacheId> {
        let prefix_key = PrefixKey::new(key.clone());
        let inserted = self.inner.insert_clean_if_absent(key, value);
        if inserted.is_some() {
            let _ = self.updates_tx.send(prefix_key);
        }
        inserted
    }

    pub fn flush_batch(&self, limit: usize) -> FlushBatch<Vec<u8>, V> {
        self.inner.flush_batch(limit)
    }

    pub fn mark_flushed(&self, batch: &FlushBatch<Vec<u8>, V>) -> usize {
        let marked = self.inner.mark_flushed(batch);
        if marked == 0 {
            return 0;
        }

        let mut candidates = HashSet::new();
        for record in batch.iter() {
            candidates.insert(record.key.clone());
        }

        if candidates.is_empty() {
            return marked;
        }

        let mut trie = self.prefix_trie.write();
        for key in candidates {
            if self.inner.read(key.as_slice(), |_, _, _, _| ()).is_none() {
                trie.remove(key.as_slice());
            }
        }

        marked
    }

    /// Drain up to `max_items` pending keys and apply them to the prefix trie.
    ///
    /// Returns the number of unique keys applied to trie state.
    pub fn advance_trie(&self, max_items: usize) -> usize {
        if max_items == 0 {
            return 0;
        }

        let rx = self.updates_rx.lock();
        let mut drained = Vec::with_capacity(max_items.min(1024));
        while drained.len() < max_items {
            match rx.try_recv() {
                Ok(Some(key)) => drained.push(key),
                _ => break,
            }
        }
        drop(rx);

        if drained.is_empty() {
            return 0;
        }

        drained.sort_unstable();
        drained.dedup();

        let mut trie = self.prefix_trie.write();
        let mut applied = 0_usize;
        for key in drained {
            trie.insert(key, ());
            applied += 1;
        }
        applied
    }

    fn snapshot_prefix_keys(&self, prefix: &[u8], limit: usize) -> Vec<PrefixKey> {
        if limit == 0 {
            return Vec::new();
        }

        let trie = self.prefix_trie.read();
        let mut keys = Vec::with_capacity(limit.min(1024));
        for (key, _) in trie.iter_prefix(prefix) {
            if keys.len() >= limit {
                break;
            }
            keys.push((*key).clone());
        }
        keys
    }

    pub fn for_each_prefix_key(
        &self,
        prefix: &[u8],
        mut reader: impl FnMut(&Vec<u8>),
        limit: usize,
    ) -> usize {
        let mut matched = 0_usize;
        for key in self.snapshot_prefix_keys(prefix, limit) {
            if self
                .inner
                .read(key.as_slice(), |k, _, _, _| reader(k))
                .is_some()
            {
                matched += 1;
            }
        }
        matched
    }

    pub fn list_prefix<R>(
        &self,
        prefix: &[u8],
        mut reader: impl FnMut(&Vec<u8>, &V, EntryState, VisibleRef) -> R,
        limit: usize,
    ) -> Vec<R> {
        let keys = self.snapshot_prefix_keys(prefix, limit);
        let mut out = Vec::with_capacity(keys.len());
        for key in keys {
            if let Some(item) = self
                .inner
                .read(key.as_slice(), |k, v, s, vr| reader(k, v, s, vr))
            {
                out.push(item);
            }
        }
        out
    }
}

#[cfg(all(test, not(feature = "loom")))]
mod tests {
    use super::*;

    #[test]
    fn mark_flushed_prunes_non_visible_keys_from_trie() {
        let map = BytePrefixMap::<usize>::new(CacheLogConfig::new(512, 512, 512));
        for i in 0..128usize {
            map.insert_dirty(format!("ab:{i:03}").into_bytes(), i);
        }

        let _ = map.advance_trie(512);
        assert_eq!(map.prefix_trie.read().count(), 128);

        let batch = map.flush_batch(128);
        assert_eq!(batch.len(), 128);
        assert_eq!(map.mark_flushed(&batch), 128);
        assert_eq!(map.prefix_trie.read().count(), 0);
    }

    #[test]
    fn mark_flushed_keeps_key_when_newer_visible_value_exists() {
        let map = BytePrefixMap::<usize>::new(CacheLogConfig::new(64, 64, 64));

        map.insert_dirty(b"ab:001".to_vec(), 1);
        let _ = map.advance_trie(64);
        let first = map.flush_batch(1);
        assert_eq!(first.len(), 1);

        map.insert_dirty(b"ab:001".to_vec(), 2);
        let _ = map.advance_trie(64);
        assert_eq!(map.prefix_trie.read().count(), 1);

        assert_eq!(map.mark_flushed(&first), 1);
        assert_eq!(map.prefix_trie.read().count(), 1);
    }
}
