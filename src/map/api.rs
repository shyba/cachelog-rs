use super::*;

impl<K, V, H> CacheLogMap<K, V, H>
where
    K: Clone + Eq + Hash,
    H: BuildHasher + Clone,
{
    pub fn visible_len(&self) -> usize {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => self.visible.len(),
            DirtyWriteMode::CoalescedMap => {
                self.clean_count.load(Ordering::Relaxed) + self.coalesced.visible_key_count()
            }
        }
    }

    pub fn dirty_log_len(&self) -> usize {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => self.dirty_mode.len(),
            DirtyWriteMode::CoalescedMap => self.coalesced.dirty_len(),
        }
    }

    pub fn dirty_backlog_counts(&self) -> DirtyBacklogCounts {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => DirtyBacklogCounts {
                dirty_total: self.dirty_mode.len(),
                pending_visible: self.dirty_mode.pending_len(),
                inflight: self.dirty_mode.inflight_len(),
                draining: 0,
            },
            DirtyWriteMode::CoalescedMap => self.coalesced.backlog_counts(),
        }
    }

    pub fn low_level(&self) -> LowLevelMap<'_, K, V, H> {
        LowLevelMap { map: self }
    }

    pub fn contains<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.read(key, |_, _, _| ()).is_some()
    }

    pub fn get_cloned<Q>(&self, key: &Q) -> Option<(V, EntryState)>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
        V: Clone,
    {
        self.read(key, |_, value, state| (value.clone(), state))
    }

    pub fn read<Q, R>(&self, key: &Q, reader: impl FnOnce(&K, &V, EntryState) -> R) -> Option<R>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.read_full(key, |stored_key, value, state, _visible| {
            reader(stored_key, value, state)
        })
    }

    pub fn for_each_prefix(
        &self,
        prefix: &[u8],
        mut reader: impl FnMut(&K, &V, EntryState),
        limit: usize,
    ) -> usize
    where
        K: Borrow<[u8]>,
    {
        if limit == 0 {
            return 0;
        }
        let keys = self.snapshot_prefix_keys_sorted(prefix, limit);
        let mut matched = 0_usize;
        for key in keys {
            if self.read(key.borrow(), |k, v, s| reader(k, v, s)).is_some() {
                matched += 1;
            }
        }
        matched
    }

    pub fn for_each_prefix_key(
        &self,
        prefix: &[u8],
        mut reader: impl FnMut(&K),
        limit: usize,
    ) -> usize
    where
        K: Borrow<[u8]>,
    {
        if limit == 0 {
            return 0;
        }
        let keys = self.snapshot_prefix_keys_sorted(prefix, limit);
        let mut matched = 0_usize;
        for key in keys {
            if self.read(key.borrow(), |k, _, _| reader(k)).is_some() {
                matched += 1;
            }
        }
        matched
    }

    pub fn list_prefix<R>(
        &self,
        prefix: &[u8],
        mut reader: impl FnMut(&K, &V, EntryState) -> R,
        limit: usize,
    ) -> Vec<R>
    where
        K: Borrow<[u8]>,
    {
        if limit == 0 {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(limit);
        self.for_each_prefix(
            prefix,
            |key, value, state| {
                out.push(reader(key, value, state));
            },
            limit,
        );
        out
    }

    pub fn put(&self, key: K, value: V) {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => self.strict_engine().put(key, value),
            DirtyWriteMode::CoalescedMap => self.coalesced_engine().put(key, value),
        }
    }

    pub fn put_batch(&self, entries: Vec<(K, V)>) -> usize {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => self.strict_engine().put_batch(entries),
            DirtyWriteMode::CoalescedMap => self.coalesced_engine().put_batch(entries),
        }
    }

    /// Materialize a stable sorted snapshot of currently visible matching keys
    /// and pass it to the caller.
    ///
    /// This is the prefix/list read boundary: it does not flush dirty data.
    pub fn with_prefix_snapshot<R>(
        &self,
        prefix: &[u8],
        limit: usize,
        reader: impl FnOnce(&[K]) -> R,
    ) -> R
    where
        K: Borrow<[u8]>,
    {
        let keys = self.snapshot_prefix_keys_sorted(prefix, limit);
        reader(keys.as_slice())
    }

    pub fn insert_clean_if_absent(&self, key: K, value: V) -> Option<CacheId> {
        let id = self.next_cache.fetch_add(1, Ordering::Relaxed);
        let record = Arc::new(CleanRecord { id, key, value });
        let visible = VisibleValue::Clean(record.clone());
        let _coalesced_publish = match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => None,
            DirtyWriteMode::CoalescedMap => Some(lock(&self.coalesced.active_publish)),
        };
        if _coalesced_publish.is_some()
            && self
                .coalesced
                .read_visible_ref(record.key.borrow())
                .is_some()
        {
            return None;
        }
        let inserted = match self.visible.entry_sync(record.key.clone()) {
            MapEntry::Vacant(vacant) => {
                vacant.insert_entry(visible);
                self.clean_count.fetch_add(1, Ordering::Relaxed);
                true
            }
            MapEntry::Occupied(_) => false,
        };
        if inserted {
            self.enqueue_clean(record.key.clone(), id);
            Some(id)
        } else {
            None
        }
    }

    pub fn evict_clean<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.visible
            .remove_if_sync(key, |visible| matches!(visible, VisibleValue::Clean(_)))
            .map(|_| {
                self.clean_count.fetch_sub(1, Ordering::Relaxed);
                true
            })
            .unwrap_or(false)
    }

    pub fn cleanup_stale_visible<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        let Some(visible_ref) = self.visible_ref(key) else {
            return false;
        };
        self.cleanup_stale_visible_matching(key, visible_ref)
    }

    /// Flush pending dirty data through the common persist callback surface.
    ///
    /// The callback receives a [`PersistBatch`] with key/value access only.
    /// Use [`CacheLogMap::low_level`] only when the caller needs the id-bearing
    /// flush path.
    pub fn with_flush_batch<E>(
        &self,
        limit: usize,
        persist: impl FnOnce(&PersistBatch<K, V>) -> Result<(), E>,
    ) -> Result<usize, E> {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => self.strict_engine().with_flush_batch(limit, persist),
            DirtyWriteMode::CoalescedMap => {
                self.coalesced_engine().with_flush_batch(limit, persist)
            }
        }
    }

    /// Repeatedly flush pending dirty data through the common persist callback
    /// surface until no backlog remains.
    pub fn flush_now<E>(
        &self,
        limit: usize,
        persist: impl FnMut(&PersistBatch<K, V>) -> Result<(), E>,
    ) -> Result<usize, E> {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => self.strict_engine().flush_now(limit, persist),
            DirtyWriteMode::CoalescedMap => self.coalesced_engine().flush_now(limit, persist),
        }
    }

    /// Flush pending dirty data, then run the provided scan.
    ///
    /// This is the persisted-scan boundary for callers that need their scan to
    /// agree with the durable store rather than the overlay alone.
    /// Flush pending dirty data through the common persist callback surface,
    /// then run a scan that must agree with persisted state.
    pub fn with_persisted_scan<R, E>(
        &self,
        flush_limit: usize,
        persist: impl FnMut(&PersistBatch<K, V>) -> Result<(), E>,
        scan: impl FnOnce() -> Result<R, E>,
    ) -> Result<R, E> {
        match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => {
                self.strict_engine()
                    .with_persisted_scan(flush_limit, persist, scan)
            }
            DirtyWriteMode::CoalescedMap => {
                self.coalesced_engine()
                    .with_persisted_scan(flush_limit, persist, scan)
            }
        }
    }

    #[cfg(not(feature = "loom"))]
    pub fn start_background_flush(
        self: &Arc<Self>,
        config: BackgroundFlushConfig,
        persist: impl Fn(&PersistBatch<K, V>) -> Result<(), String> + Send + Sync + 'static,
    ) -> BackgroundFlushHandle
    where
        K: Send + Sync + 'static,
        V: Send + Sync + 'static,
        H: Send + Sync + 'static,
    {
        let service = match self.dirty_write_mode {
            DirtyWriteMode::StrictLog => self.strict_arc_engine().background_flush_service(persist),
            DirtyWriteMode::CoalescedMap => self
                .coalesced_arc_engine()
                .background_flush_service(persist),
        };
        BackgroundFlushHandle::start_with_service(config, service)
    }

    #[cfg(not(feature = "loom"))]
    pub fn start_background_flush_default(
        self: &Arc<Self>,
        persist: impl Fn(&PersistBatch<K, V>) -> Result<(), String> + Send + Sync + 'static,
    ) -> BackgroundFlushHandle
    where
        K: Send + Sync + 'static,
        V: Send + Sync + 'static,
        H: Send + Sync + 'static,
    {
        self.start_background_flush(BackgroundFlushConfig::default(), persist)
    }

    #[cfg(not(feature = "loom"))]
    pub fn start_background_flush_for_batch_size(
        self: &Arc<Self>,
        batch_size: usize,
        persist: impl Fn(&PersistBatch<K, V>) -> Result<(), String> + Send + Sync + 'static,
    ) -> BackgroundFlushHandle
    where
        K: Send + Sync + 'static,
        V: Send + Sync + 'static,
        H: Send + Sync + 'static,
    {
        self.start_background_flush(BackgroundFlushConfig::for_batch_size(batch_size), persist)
    }
}
