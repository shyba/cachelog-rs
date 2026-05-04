use std::borrow::Borrow;
use std::cmp::Ordering;
use std::collections::HashSet;
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::hash::{Hash, Hasher};

use parking_lot::{Mutex, RwLock};
use qp_trie::Trie;

use crossbeam_channel::{Receiver, Sender};

#[cfg(not(feature = "loom"))]
use crate::background_flush::BackgroundFlushService;
use crate::low_level::{CacheId, FlushBatch, VisibleRef, WriteId};
use crate::sync::Arc;
#[cfg(not(feature = "loom"))]
use crate::{BackgroundFlushConfig, BackgroundFlushHandle};
use crate::{CacheLogConfig, CacheLogMap, EntryState, PersistBatch};

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
///
/// Raw mutable access to the wrapped [`CacheLogMap`] is intentionally not
/// exposed, because writes must flow through this wrapper to keep prefix scans
/// in sync.
///
/// ```compile_fail
/// use cachelog::{BytePrefixMap, CacheLogConfig};
///
/// let map = BytePrefixMap::<usize>::new(CacheLogConfig::new(64, 64, 64));
/// map.inner().insert_dirty(b"ab:001".to_vec(), 1);
/// ```
pub struct BytePrefixMap<V, H = RandomState>
where
    H: BuildHasher + Clone,
{
    inner: Arc<CacheLogMap<Vec<u8>, V, H>>,
    prefix_trie: Arc<RwLock<Trie<PrefixKey, ()>>>,
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
        let (updates_tx, updates_rx) = crossbeam_channel::unbounded();
        Self {
            inner: Arc::new(CacheLogMap::with_hasher(config, build_hasher)),
            prefix_trie: Arc::new(RwLock::new(Trie::new())),
            updates_tx,
            updates_rx: Mutex::new(updates_rx),
        }
    }

    /// Mirror of the wrapped map's visible-entry count.
    pub fn visible_len(&self) -> usize {
        self.inner.visible_len()
    }

    /// Mirror of the wrapped map's dirty-log count.
    pub fn dirty_log_len(&self) -> usize {
        self.inner.dirty_log_len()
    }

    /// Mirror of the wrapped map's clean-store count.
    pub fn clean_store_len(&self) -> usize {
        self.inner.clean_store_len()
    }

    /// Mirror of [`CacheLogMap::read`], preserving the product read API.
    pub fn read<R>(
        &self,
        key: &[u8],
        reader: impl FnOnce(&Vec<u8>, &V, EntryState) -> R,
    ) -> Option<R> {
        self.inner.read(key, reader)
    }

    /// Mirror of the low-level read path that exposes the wrapped entry ref.
    pub fn read_full<R>(
        &self,
        key: &[u8],
        reader: impl FnOnce(&Vec<u8>, &V, EntryState, VisibleRef) -> R,
    ) -> Option<R> {
        self.inner.low_level().read_full(key, reader)
    }

    /// Mirror of the wrapped product write API, and enqueues the key for trie
    /// publication.
    pub fn put(&self, key: Vec<u8>, value: V) {
        let prefix_key = PrefixKey::new(key.clone());
        self.inner.put(key, value);
        let _ = self.updates_tx.send(prefix_key);
    }

    /// Batch form of [`put`]; keys are enqueued for trie publication after the
    /// wrapped map accepts the batch.
    pub fn put_batch(&self, entries: Vec<(Vec<u8>, V)>) -> usize {
        if entries.is_empty() {
            return 0;
        }

        let mut prefix_keys = Vec::with_capacity(entries.len());
        for (key, _) in &entries {
            prefix_keys.push(PrefixKey::new(key.clone()));
        }

        let written = self.inner.put_batch(entries);
        for prefix_key in prefix_keys {
            let _ = self.updates_tx.send(prefix_key);
        }
        written
    }

    /// Low-level dirty insert that also publishes the key to the trie queue.
    pub fn insert_dirty(&self, key: Vec<u8>, value: V) -> WriteId {
        let prefix_key = PrefixKey::new(key.clone());
        let id = self.inner.low_level().insert_dirty(key, value);
        let _ = self.updates_tx.send(prefix_key);
        id
    }

    /// Alias for [`insert_dirty`].
    pub fn upsert_dirty(&self, key: Vec<u8>, value: V) -> WriteId {
        self.insert_dirty(key, value)
    }

    /// Mirror of the wrapped clean insert, with trie publication when it wins.
    pub fn insert_clean_if_absent(&self, key: Vec<u8>, value: V) -> Option<CacheId> {
        let prefix_key = PrefixKey::new(key.clone());
        let inserted = self.inner.insert_clean_if_absent(key, value);
        if inserted.is_some() {
            let _ = self.updates_tx.send(prefix_key);
        }
        inserted
    }

    /// Acquire a low-level id-bearing flush batch from the wrapped cachelog.
    pub fn flush_batch(&self, limit: usize) -> FlushBatch<Vec<u8>, V> {
        self.inner.low_level().flush_batch(limit)
    }

    /// Flush pending dirty data through the common persist callback surface.
    pub fn with_flush_batch<E>(
        &self,
        limit: usize,
        persist: impl FnOnce(&PersistBatch<Vec<u8>, V>) -> Result<(), E>,
    ) -> Result<usize, E> {
        Self::flush_once_with_persist(&self.inner, &self.prefix_trie, limit, persist)
    }

    /// Repeatedly flush pending dirty data through the common persist callback
    /// surface until no backlog remains.
    pub fn flush_now<E>(
        &self,
        limit: usize,
        mut persist: impl FnMut(&PersistBatch<Vec<u8>, V>) -> Result<(), E>,
    ) -> Result<usize, E> {
        Self::flush_until_empty(&self.inner, &self.prefix_trie, limit, &mut persist)
    }

    /// Flush pending dirty data through the common persist callback surface,
    /// then run a scan that must agree with persisted state.
    pub fn with_persisted_scan<R, E>(
        &self,
        flush_limit: usize,
        persist: impl FnMut(&PersistBatch<Vec<u8>, V>) -> Result<(), E>,
        scan: impl FnOnce() -> Result<R, E>,
    ) -> Result<R, E> {
        let _ = self.flush_now(flush_limit, persist)?;
        scan()
    }

    /// Complete a low-level id-bearing flush batch previously acquired via
    /// [`flush_batch`]. This is trie-preserving: it only removes keys once the
    /// wrapped map no longer exposes a visible value for them.
    pub fn mark_flushed(&self, batch: &FlushBatch<Vec<u8>, V>) -> usize {
        Self::mark_flushed_inner(&self.inner, &self.prefix_trie, batch)
    }

    fn flush_once_with_persist<E>(
        inner: &CacheLogMap<Vec<u8>, V, H>,
        prefix_trie: &RwLock<Trie<PrefixKey, ()>>,
        limit: usize,
        persist: impl FnOnce(&PersistBatch<Vec<u8>, V>) -> Result<(), E>,
    ) -> Result<usize, E> {
        let batch = inner.low_level().flush_batch(limit.max(1));
        if batch.is_empty() {
            return Ok(0);
        }
        let persist_batch = PersistBatch::from_flush_batch(batch.clone());
        persist(&persist_batch)?;
        Ok(Self::mark_flushed_inner(inner, prefix_trie, &batch))
    }

    fn flush_until_empty<E>(
        inner: &CacheLogMap<Vec<u8>, V, H>,
        prefix_trie: &RwLock<Trie<PrefixKey, ()>>,
        limit: usize,
        persist: &mut impl FnMut(&PersistBatch<Vec<u8>, V>) -> Result<(), E>,
    ) -> Result<usize, E> {
        let mut total = 0_usize;
        loop {
            let batch = inner.low_level().flush_batch(limit.max(1));
            if batch.is_empty() {
                return Ok(total);
            }
            total += Self::persist_and_mark_batch(inner, prefix_trie, &batch, persist)?;
        }
    }

    fn persist_and_mark_batch<E>(
        inner: &CacheLogMap<Vec<u8>, V, H>,
        prefix_trie: &RwLock<Trie<PrefixKey, ()>>,
        batch: &FlushBatch<Vec<u8>, V>,
        persist: &mut impl FnMut(&PersistBatch<Vec<u8>, V>) -> Result<(), E>,
    ) -> Result<usize, E> {
        let persist_batch = PersistBatch::from_flush_batch(batch.clone());
        persist(&persist_batch)?;
        Ok(Self::mark_flushed_inner(inner, prefix_trie, batch))
    }

    fn mark_flushed_inner(
        inner: &CacheLogMap<Vec<u8>, V, H>,
        prefix_trie: &RwLock<Trie<PrefixKey, ()>>,
        batch: &FlushBatch<Vec<u8>, V>,
    ) -> usize {
        let marked = inner.low_level().mark_flushed(batch);
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

        let mut trie = prefix_trie.write();
        for key in candidates {
            if inner.read(key.as_slice(), |_, _, _| ()).is_none() {
                trie.remove(key.as_slice());
            }
        }

        marked
    }

    /// Drain up to `max_items` pending keys and apply them to the prefix trie.
    ///
    /// Key/value writes are enqueued to an internal channel immediately on
    /// [`put`][Self::put] / [`insert_dirty`][Self::insert_dirty], but the prefix
    /// trie is updated lazily by this method. That decouples trie visibility
    /// from dirty-write visibility: a key can be flushed before its prefix is
    /// visible in the trie, and vice versa.
    ///
    /// Keys are sorted and deduplicated before insertion, so redundant writes
    /// are collapsed into a single trie entry.
    ///
    /// Call this periodically from a maintenance thread when idle. It is
    /// side-effect-free with respect to persisted state.
    ///
    /// Returns the number of unique keys inserted into the trie.
    pub fn advance_trie(&self, max_items: usize) -> usize {
        if max_items == 0 {
            return 0;
        }

        let rx = self.updates_rx.lock();
        let mut drained = Vec::with_capacity(max_items.min(1024));
        while drained.len() < max_items {
            match rx.try_recv() {
                Ok(key) => drained.push(key),
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

    /// Visit matching trie keys and call `reader` for each visible entry.
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
                .read(key.as_slice(), |k, _, _| reader(k))
                .is_some()
            {
                matched += 1;
            }
        }
        matched
    }

    /// Collect prefix matches into a vector.
    ///
    /// Compared with `CacheLogMap`, this is trie-driven and only sees keys
    /// that have been published via [`advance_trie`]. It does not flush dirty
    /// state.
    pub fn list_prefix<R>(
        &self,
        prefix: &[u8],
        mut reader: impl FnMut(&Vec<u8>, &V, EntryState) -> R,
        limit: usize,
    ) -> Vec<R> {
        let keys = self.snapshot_prefix_keys(prefix, limit);
        let mut out = Vec::with_capacity(keys.len());
        for key in keys {
            if let Some(item) = self.inner.read(key.as_slice(), |k, v, s| reader(k, v, s)) {
                out.push(item);
            }
        }
        out
    }

    #[cfg(not(feature = "loom"))]
    /// Start the background flush service with an explicit config.
    ///
    /// This is a BytePrefixMap-specific wrapper around the wrapped map's flush
    /// loop; it keeps the prefix trie in sync when batches are acknowledged.
    pub fn start_background_flush(
        &self,
        config: BackgroundFlushConfig,
        persist: impl Fn(&PersistBatch<Vec<u8>, V>) -> Result<(), String> + Send + Sync + 'static,
    ) -> BackgroundFlushHandle
    where
        V: Send + Sync + 'static,
        H: Send + Sync + 'static,
    {
        self.start_background_flush_with_config(config, persist)
    }

    #[cfg(not(feature = "loom"))]
    /// Start background flush with the default config.
    pub fn start_background_flush_default(
        &self,
        persist: impl Fn(&PersistBatch<Vec<u8>, V>) -> Result<(), String> + Send + Sync + 'static,
    ) -> BackgroundFlushHandle
    where
        V: Send + Sync + 'static,
        H: Send + Sync + 'static,
    {
        self.start_background_flush_with_config(BackgroundFlushConfig::default(), persist)
    }

    #[cfg(not(feature = "loom"))]
    /// Start background flush using a batch-size-derived config.
    pub fn start_background_flush_for_batch_size(
        &self,
        batch_size: usize,
        persist: impl Fn(&PersistBatch<Vec<u8>, V>) -> Result<(), String> + Send + Sync + 'static,
    ) -> BackgroundFlushHandle
    where
        V: Send + Sync + 'static,
        H: Send + Sync + 'static,
    {
        self.start_background_flush_with_config(
            BackgroundFlushConfig::for_batch_size(batch_size),
            persist,
        )
    }

    #[cfg(not(feature = "loom"))]
    fn start_background_flush_with_config(
        &self,
        config: BackgroundFlushConfig,
        persist: impl Fn(&PersistBatch<Vec<u8>, V>) -> Result<(), String> + Send + Sync + 'static,
    ) -> BackgroundFlushHandle
    where
        V: Send + Sync + 'static,
        H: Send + Sync + 'static,
    {
        let persist = Arc::new(persist);
        let dirty_inner = self.inner.clone();
        let dirty_len: Arc<dyn Fn() -> usize + Send + Sync> =
            Arc::new(move || dirty_inner.dirty_log_len());

        let flush_inner = self.inner.clone();
        let flush_trie = self.prefix_trie.clone();
        let flush: Arc<dyn Fn(usize) -> Result<(), String> + Send + Sync> =
            Arc::new(move |limit| {
                Self::flush_now_inner(&flush_inner, &flush_trie, limit, |batch| persist(batch))
                    .map(|_| ())
            });

        BackgroundFlushHandle::start_with_service(
            config,
            BackgroundFlushService { dirty_len, flush },
        )
    }

    #[cfg(not(feature = "loom"))]
    fn flush_now_inner<E>(
        inner: &CacheLogMap<Vec<u8>, V, H>,
        prefix_trie: &RwLock<Trie<PrefixKey, ()>>,
        limit: usize,
        mut persist: impl FnMut(&PersistBatch<Vec<u8>, V>) -> Result<(), E>,
    ) -> Result<usize, E> {
        Self::flush_until_empty(inner, prefix_trie, limit, &mut persist)
    }
}

#[cfg(all(test, not(feature = "loom")))]
mod tests {
    use super::*;
    use std::sync::{Arc as StdArc, Mutex as StdMutex};

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

    #[test]
    fn with_persisted_scan_flushes_and_prunes_trie() {
        let map = BytePrefixMap::<usize>::new(CacheLogConfig::new(64, 64, 64));
        map.insert_dirty(b"ab:001".to_vec(), 1);
        map.insert_dirty(b"ab:002".to_vec(), 2);
        let _ = map.advance_trie(64);

        let mut persisted = Vec::new();
        let result = map
            .with_persisted_scan(
                8,
                |batch| {
                    persisted = batch
                        .iter()
                        .map(|entry| (entry.key().clone(), *entry.value()))
                        .collect();
                    Ok::<_, ()>(())
                },
                || Ok::<_, ()>(map.list_prefix(b"ab:", |k, _, _| k.clone(), 16)),
            )
            .expect("persisted scan");

        assert_eq!(
            persisted,
            vec![(b"ab:001".to_vec(), 1), (b"ab:002".to_vec(), 2)]
        );
        assert!(
            result.is_empty(),
            "flushed trie-backed list should be empty"
        );
        assert_eq!(map.prefix_trie.read().count(), 0);
    }

    #[test]
    fn start_background_flush_for_batch_size_flushes_pending_writes() {
        let map = BytePrefixMap::<usize>::new(CacheLogConfig::new(64, 64, 64));
        map.insert_dirty(b"ab:001".to_vec(), 1);
        map.insert_dirty(b"ab:002".to_vec(), 2);
        assert_eq!(map.advance_trie(16), 2);

        let persisted = StdArc::new(StdMutex::new(Vec::new()));
        let persisted_flush = StdArc::clone(&persisted);
        let service = map.start_background_flush_for_batch_size(2, move |batch| {
            persisted_flush
                .lock()
                .expect("persisted lock")
                .push(batch.len());
            Ok(())
        });

        service.note_writes(2).expect("wake worker");
        service.flush_sync().expect("flush sync");

        let lens = persisted.lock().expect("persisted lock").clone();
        assert_eq!(lens.iter().sum::<usize>(), 2, "persisted lens: {lens:?}");
        assert_eq!(map.prefix_trie.read().count(), 0);
        assert!(
            map.list_prefix(b"ab:", |key, _, _| key.clone(), 1)
                .is_empty()
        );
    }
}
