#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum DirtyAllocMode {
    #[default]
    OwnedPerWrite,
    PooledVec,
    ChunkedArena,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum DirtyQueueBackend {
    #[default]
    Kanal,
    Crossbeam,
    StdSync,
}

#[cfg(feature = "loom")]
mod imp {
    use std::collections::VecDeque;

    use crate::dirty_mode::{DirtyAllocMode, DirtyQueueBackend};
    use crate::entry::{DirtyRecord, FlushBatch, WriteId};
    use crate::sync::{Arc, Mutex, lock, new_mutex};

    pub(crate) trait DirtyMode<K, V> {
        type VisibleDirty: Clone;

        fn new(
            capacity: usize,
            queue_backend: DirtyQueueBackend,
            alloc_mode: DirtyAllocMode,
        ) -> Self;
        fn append(&self, key: K, value: V) -> Self::VisibleDirty;
        fn append_batch(&self, entries: Vec<(K, V)>) -> Vec<Self::VisibleDirty>;
        fn flush_batch(&self, limit: usize) -> FlushBatch<K, V>;
        fn mark_flushed(&self, batch: &FlushBatch<K, V>) -> usize;
        fn len(&self) -> usize;

        #[cfg(any(test, feature = "loom"))]
        fn pending_records(&self) -> Vec<Self::VisibleDirty>;

        #[cfg(any(test, feature = "loom"))]
        fn inflight_records(&self) -> Vec<Self::VisibleDirty>;

        #[cfg(any(test, feature = "loom"))]
        fn next_write(&self) -> WriteId;
    }

    struct DirtyLog<K, V> {
        next_id: WriteId,
        entries: VecDeque<Arc<DirtyRecord<K, V>>>,
        inflight: Option<FlushBatch<K, V>>,
    }

    impl<K, V> DirtyLog<K, V> {
        fn with_capacity(capacity: usize) -> Self {
            Self {
                next_id: 0,
                entries: VecDeque::with_capacity(capacity.max(1)),
                inflight: None,
            }
        }
    }

    pub(crate) struct OrderedFifoDirty<K, V> {
        inner: Mutex<DirtyLog<K, V>>,
    }

    impl<K, V> DirtyMode<K, V> for OrderedFifoDirty<K, V> {
        type VisibleDirty = Arc<DirtyRecord<K, V>>;

        fn new(
            capacity: usize,
            _queue_backend: DirtyQueueBackend,
            _alloc_mode: DirtyAllocMode,
        ) -> Self {
            Self {
                inner: new_mutex(DirtyLog::with_capacity(capacity)),
            }
        }

        fn append(&self, key: K, value: V) -> Self::VisibleDirty {
            let mut inner = lock(&self.inner);
            let id = inner.next_id;
            inner.next_id = inner.next_id.wrapping_add(1);
            let record = Arc::new(DirtyRecord { id, key, value });
            inner.entries.push_back(record.clone());
            record
        }

        fn append_batch(&self, entries: Vec<(K, V)>) -> Vec<Self::VisibleDirty> {
            if entries.is_empty() {
                return Vec::new();
            }
            let mut inner = lock(&self.inner);
            let mut out = Vec::with_capacity(entries.len());
            for (key, value) in entries {
                let id = inner.next_id;
                inner.next_id = inner.next_id.wrapping_add(1);
                let record = Arc::new(DirtyRecord { id, key, value });
                inner.entries.push_back(record.clone());
                out.push(record);
            }
            out
        }

        fn flush_batch(&self, limit: usize) -> FlushBatch<K, V> {
            let mut inner = lock(&self.inner);
            if let Some(batch) = &inner.inflight {
                return batch.clone();
            }
            let mut entries = Vec::with_capacity(limit);
            while entries.len() < limit {
                let Some(record) = inner.entries.pop_front() else {
                    break;
                };
                entries.push(record);
            }
            let batch = FlushBatch::new(entries);
            if !batch.is_empty() {
                inner.inflight = Some(batch.clone());
            }
            batch
        }

        fn mark_flushed(&self, batch: &FlushBatch<K, V>) -> usize {
            let mut inner = lock(&self.inner);
            let Some(inflight) = &inner.inflight else {
                return 0;
            };
            if inflight.len() != batch.len() || inflight.last_id() != batch.last_id() {
                return 0;
            }
            inner
                .inflight
                .take()
                .expect("inflight batch vanished")
                .len()
        }

        fn len(&self) -> usize {
            let inner = lock(&self.inner);
            inner.entries.len() + inner.inflight.as_ref().map_or(0, FlushBatch::len)
        }

        #[cfg(any(test, feature = "loom"))]
        fn pending_records(&self) -> Vec<Self::VisibleDirty> {
            lock(&self.inner).entries.iter().cloned().collect()
        }

        #[cfg(any(test, feature = "loom"))]
        fn inflight_records(&self) -> Vec<Self::VisibleDirty> {
            lock(&self.inner)
                .inflight
                .as_ref()
                .map(|batch| batch.entries.to_vec())
                .unwrap_or_default()
        }

        #[cfg(any(test, feature = "loom"))]
        fn next_write(&self) -> WriteId {
            lock(&self.inner).next_id
        }
    }
}

#[cfg(not(feature = "loom"))]
mod imp {
    use std::collections::VecDeque;
    use std::sync::mpsc;

    use crossbeam_channel as cbch;
    use kanal::{Receiver as KanalReceiver, Sender as KanalSender};

    use crate::dirty_mode::{DirtyAllocMode, DirtyQueueBackend};
    #[cfg(any(test, feature = "loom"))]
    use crate::entry::WriteId;
    use crate::entry::{DirtyRecord, FlushBatch};
    use crate::sync::{Arc, AtomicU64, AtomicUsize, CachePadded, Mutex, Ordering, lock, new_mutex};

    pub(crate) trait DirtyMode<K, V> {
        type VisibleDirty: Clone;

        fn new(
            capacity: usize,
            queue_backend: DirtyQueueBackend,
            alloc_mode: DirtyAllocMode,
        ) -> Self;
        fn append(&self, key: K, value: V) -> Self::VisibleDirty;
        fn append_batch(&self, entries: Vec<(K, V)>) -> Vec<Self::VisibleDirty>;
        fn flush_batch(&self, limit: usize) -> FlushBatch<K, V>;
        fn mark_flushed(&self, batch: &FlushBatch<K, V>) -> usize;
        fn len(&self) -> usize;

        #[cfg(any(test, feature = "loom"))]
        fn pending_records(&self) -> Vec<Self::VisibleDirty>;

        #[cfg(any(test, feature = "loom"))]
        fn inflight_records(&self) -> Vec<Self::VisibleDirty>;

        #[cfg(any(test, feature = "loom"))]
        fn next_write(&self) -> WriteId;
    }

    enum PendingItem<K, V> {
        One(Arc<DirtyRecord<K, V>>),
        BatchVec(Vec<Arc<DirtyRecord<K, V>>>),
        BatchSlice(Box<[Arc<DirtyRecord<K, V>>]>),
    }

    enum DirtyQueue<K, V> {
        Kanal {
            tx: KanalSender<PendingItem<K, V>>,
            rx: Mutex<KanalReceiver<PendingItem<K, V>>>,
        },
        Crossbeam {
            tx: cbch::Sender<PendingItem<K, V>>,
            rx: Mutex<cbch::Receiver<PendingItem<K, V>>>,
        },
        StdSync {
            tx: mpsc::SyncSender<PendingItem<K, V>>,
            rx: Mutex<mpsc::Receiver<PendingItem<K, V>>>,
        },
    }

    impl<K, V> DirtyQueue<K, V> {
        fn new(capacity: usize, backend: DirtyQueueBackend) -> Self {
            match backend {
                DirtyQueueBackend::Kanal => {
                    let (tx, rx) = kanal::unbounded();
                    Self::Kanal {
                        tx,
                        rx: new_mutex(rx),
                    }
                }
                DirtyQueueBackend::Crossbeam => {
                    let (tx, rx) = cbch::bounded(capacity.max(1));
                    Self::Crossbeam {
                        tx,
                        rx: new_mutex(rx),
                    }
                }
                DirtyQueueBackend::StdSync => {
                    let (tx, rx) = mpsc::sync_channel(capacity.max(1));
                    Self::StdSync {
                        tx,
                        rx: new_mutex(rx),
                    }
                }
            }
        }

        fn send_one(&self, record: Arc<DirtyRecord<K, V>>) {
            self.send_item(PendingItem::One(record));
        }

        fn send_batch_vec(&self, records: Vec<Arc<DirtyRecord<K, V>>>) {
            self.send_item(PendingItem::BatchVec(records));
        }

        fn send_batch_slice(&self, records: Box<[Arc<DirtyRecord<K, V>>]>) {
            self.send_item(PendingItem::BatchSlice(records));
        }

        fn send_item(&self, item: PendingItem<K, V>) {
            match self {
                Self::Kanal { tx, .. } => tx.send(item).expect("dirty queue receiver dropped"),
                Self::Crossbeam { tx, .. } => tx.send(item).expect("dirty queue receiver dropped"),
                Self::StdSync { tx, .. } => tx.send(item).expect("dirty queue receiver dropped"),
            }
        }

        fn try_recv_item(&self) -> Option<PendingItem<K, V>> {
            match self {
                Self::Kanal { rx, .. } => match lock(rx).try_recv() {
                    Ok(Some(item)) => Some(item),
                    Ok(None) => None,
                    Err(_) => None,
                },
                Self::Crossbeam { rx, .. } => lock(rx).try_recv().ok(),
                Self::StdSync { rx, .. } => lock(rx).try_recv().ok(),
            }
        }
    }

    pub(crate) struct OrderedFifoDirty<K, V> {
        next_id: CachePadded<AtomicU64>,
        pending_len: CachePadded<AtomicUsize>,
        queue: DirtyQueue<K, V>,
        alloc_mode: DirtyAllocMode,
        append_lock: Mutex<()>,
        staged: Mutex<VecDeque<Arc<DirtyRecord<K, V>>>>,
        inflight: Mutex<Option<FlushBatch<K, V>>>,
        #[cfg(test)]
        pending_shadow: Mutex<VecDeque<Arc<DirtyRecord<K, V>>>>,
    }

    impl<K, V> DirtyMode<K, V> for OrderedFifoDirty<K, V> {
        type VisibleDirty = Arc<DirtyRecord<K, V>>;

        fn new(
            capacity: usize,
            queue_backend: DirtyQueueBackend,
            alloc_mode: DirtyAllocMode,
        ) -> Self {
            Self {
                next_id: CachePadded(AtomicU64::new(0)),
                pending_len: CachePadded(AtomicUsize::new(0)),
                queue: DirtyQueue::new(capacity, queue_backend),
                alloc_mode,
                append_lock: new_mutex(()),
                staged: new_mutex(VecDeque::new()),
                inflight: new_mutex(None),
                #[cfg(test)]
                pending_shadow: new_mutex(VecDeque::new()),
            }
        }

        fn append(&self, key: K, value: V) -> Self::VisibleDirty {
            let _guard = lock(&self.append_lock);
            let id = self.next_id.0.fetch_add(1, Ordering::Relaxed);
            let record = Arc::new(DirtyRecord { id, key, value });
            self.queue.send_one(record.clone());
            self.pending_len.0.fetch_add(1, Ordering::Relaxed);
            #[cfg(test)]
            {
                lock(&self.pending_shadow).push_back(record.clone());
            }
            record
        }

        fn append_batch(&self, entries: Vec<(K, V)>) -> Vec<Self::VisibleDirty> {
            if entries.is_empty() {
                return Vec::new();
            }
            let _guard = lock(&self.append_lock);
            let len = entries.len();
            let base = self.next_id.0.fetch_add(len as u64, Ordering::Relaxed);
            let mut out = Vec::with_capacity(len);
            for (offset, (key, value)) in entries.into_iter().enumerate() {
                let record = Arc::new(DirtyRecord {
                    id: base.wrapping_add(offset as u64),
                    key,
                    value,
                });
                out.push(record);
            }
            match self.alloc_mode {
                DirtyAllocMode::OwnedPerWrite => {
                    for record in &out {
                        self.queue.send_one(record.clone());
                    }
                }
                DirtyAllocMode::PooledVec => {
                    self.queue.send_batch_vec(out.clone());
                }
                DirtyAllocMode::ChunkedArena => {
                    self.queue.send_batch_slice(out.clone().into_boxed_slice());
                }
            }
            self.pending_len.0.fetch_add(len, Ordering::Relaxed);
            #[cfg(test)]
            {
                let mut shadow = lock(&self.pending_shadow);
                for record in &out {
                    shadow.push_back(record.clone());
                }
            }
            out
        }

        fn flush_batch(&self, limit: usize) -> FlushBatch<K, V> {
            let mut inflight = lock(&self.inflight);
            if let Some(batch) = &*inflight {
                return batch.clone();
            }
            let mut entries = Vec::with_capacity(limit);

            while entries.len() < limit {
                {
                    let mut staged = lock(&self.staged);
                    while entries.len() < limit {
                        let Some(record) = staged.pop_front() else {
                            break;
                        };
                        self.pending_len.0.fetch_sub(1, Ordering::Relaxed);
                        #[cfg(test)]
                        {
                            let _ = lock(&self.pending_shadow).pop_front();
                        }
                        entries.push(record);
                    }
                }

                if entries.len() >= limit {
                    break;
                }

                let Some(item) = self.queue.try_recv_item() else {
                    break;
                };

                match item {
                    PendingItem::One(record) => {
                        self.pending_len.0.fetch_sub(1, Ordering::Relaxed);
                        #[cfg(test)]
                        {
                            let _ = lock(&self.pending_shadow).pop_front();
                        }
                        entries.push(record);
                    }
                    PendingItem::BatchVec(records) => {
                        let mut iter = records.into_iter();
                        while entries.len() < limit {
                            let Some(record) = iter.next() else {
                                break;
                            };
                            self.pending_len.0.fetch_sub(1, Ordering::Relaxed);
                            #[cfg(test)]
                            {
                                let _ = lock(&self.pending_shadow).pop_front();
                            }
                            entries.push(record);
                        }
                        let mut staged = lock(&self.staged);
                        for record in iter {
                            staged.push_back(record);
                        }
                    }
                    PendingItem::BatchSlice(records) => {
                        let mut iter = records.into_vec().into_iter();
                        while entries.len() < limit {
                            let Some(record) = iter.next() else {
                                break;
                            };
                            self.pending_len.0.fetch_sub(1, Ordering::Relaxed);
                            #[cfg(test)]
                            {
                                let _ = lock(&self.pending_shadow).pop_front();
                            }
                            entries.push(record);
                        }
                        let mut staged = lock(&self.staged);
                        for record in iter {
                            staged.push_back(record);
                        }
                    }
                }
            }

            let batch = FlushBatch::new(entries);
            if !batch.is_empty() {
                *inflight = Some(batch.clone());
            }
            batch
        }

        fn mark_flushed(&self, batch: &FlushBatch<K, V>) -> usize {
            let mut inflight = lock(&self.inflight);
            let Some(current) = &*inflight else {
                return 0;
            };
            if current.len() != batch.len() || current.last_id() != batch.last_id() {
                return 0;
            }
            inflight.take().expect("inflight batch vanished").len()
        }

        fn len(&self) -> usize {
            let inflight_len = lock(&self.inflight).as_ref().map_or(0, FlushBatch::len);
            self.pending_len.0.load(Ordering::Relaxed) + inflight_len
        }

        #[cfg(any(test, feature = "loom"))]
        fn pending_records(&self) -> Vec<Self::VisibleDirty> {
            #[cfg(test)]
            {
                return lock(&self.pending_shadow).iter().cloned().collect();
            }
            #[allow(unreachable_code)]
            Vec::new()
        }

        #[cfg(any(test, feature = "loom"))]
        fn inflight_records(&self) -> Vec<Self::VisibleDirty> {
            lock(&self.inflight)
                .as_ref()
                .map(|batch| batch.entries.to_vec())
                .unwrap_or_default()
        }

        #[cfg(any(test, feature = "loom"))]
        fn next_write(&self) -> WriteId {
            self.next_id.0.load(Ordering::Relaxed)
        }
    }
}

pub(crate) use imp::{DirtyMode, OrderedFifoDirty};
