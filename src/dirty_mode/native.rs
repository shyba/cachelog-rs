//! Native crossbeam-channel based OrderedFifoDirty implementation (default).

use std::collections::VecDeque;

use crossbeam_channel as cbch;

use crate::dirty_mode::FlushWork;
#[cfg(any(test, feature = "loom"))]
use crate::entry::WriteId;
use crate::entry::{DirtyRecord, FlushBatch};
use crate::sync::{Arc, AtomicU64, AtomicUsize, CachePadded, Mutex, Ordering, lock, new_mutex};

pub(crate) trait DirtyMode<K, V> {
    type VisibleDirty: Clone;

    fn new(capacity: usize) -> Self;
    fn append(&self, key: K, value: V) -> Self::VisibleDirty;
    fn append_batch(&self, entries: Vec<(K, V)>) -> Vec<Self::VisibleDirty>;
    fn flush_batch(&self, limit: usize) -> FlushBatch<K, V>;
    fn wait_for_flush_work(&self, limit: usize) -> FlushWork<K, V>;
    fn signal_force_flush(&self);
    fn mark_flushed(&self, batch: &FlushBatch<K, V>) -> usize;
    fn len(&self) -> usize;

    #[cfg(any(test, feature = "loom"))]
    fn pending_records(&self) -> Vec<Self::VisibleDirty>;

    #[cfg(any(test, feature = "loom"))]
    fn inflight_records(&self) -> Vec<Self::VisibleDirty>
    where
        K: Clone,
        V: Clone;

    #[cfg(any(test, feature = "loom"))]
    fn next_write(&self) -> WriteId;
}

/// A single dirty entry or a batch thereof sent through the dirty queue.
enum PendingItem<K, V> {
    One(Arc<DirtyRecord<K, V>>),
    BatchSlice(Box<[Arc<DirtyRecord<K, V>>]>),
    /// Signals the flusher to flush immediately, bypassing the batch limit.
    ForceFlush,
}

/// The bounded sender/receiver pair for the dirty queue.
///
/// Created with a `capacity` which becomes the bound of the underlying
/// crossbeam-channel. When the channel is full, [`OrderedFifoDirty::enqueue_record`]
/// falls through to `overflow` instead of blocking.
struct DirtyQueue<K, V> {
    tx: cbch::Sender<PendingItem<K, V>>,
    rx: Mutex<cbch::Receiver<PendingItem<K, V>>>,
}

impl<K, V> DirtyQueue<K, V> {
    fn new(capacity: usize) -> Self {
        let (tx, rx) = cbch::bounded(capacity.max(1));
        Self {
            tx,
            rx: new_mutex(rx),
        }
    }

    fn try_send_item(&self, item: PendingItem<K, V>) -> Result<(), PendingItem<K, V>> {
        self.tx.try_send(item).map_err(|err| match err {
            cbch::TrySendError::Full(item) => item,
            cbch::TrySendError::Disconnected(_) => panic!("dirty queue receiver dropped"),
        })
    }

    fn send_force_flush(&self) {
        let _ = self.tx.try_send(PendingItem::ForceFlush);
    }

    fn try_recv_item(&self) -> Option<PendingItem<K, V>> {
        lock(&self.rx).try_recv().ok()
    }

    fn recv_item(&self) -> Option<PendingItem<K, V>> {
        lock(&self.rx).recv().ok()
    }
}

pub(crate) struct OrderedFifoDirty<K, V> {
    next_id: CachePadded<AtomicU64>,
    pending_len: CachePadded<AtomicUsize>,
    queue: DirtyQueue<K, V>,
    append_lock: Mutex<()>,
    staged: Mutex<VecDeque<Arc<DirtyRecord<K, V>>>>,
    /// Unbounded overflow for when the channel is full. Writes fall through here
    /// via [`enqueue_record`](Self::enqueue_record) rather than blocking, and
    /// are drained FIFO when the channel next empties.
    overflow: Mutex<VecDeque<Arc<DirtyRecord<K, V>>>>,
    inflight: Mutex<Option<FlushBatch<K, V>>>,
    #[cfg(test)]
    pending_shadow: Mutex<VecDeque<Arc<DirtyRecord<K, V>>>>,
}

impl<K, V> OrderedFifoDirty<K, V> {
    pub(crate) fn pending_len(&self) -> usize {
        self.pending_len.0.load(Ordering::Acquire)
    }

    pub(crate) fn inflight_len(&self) -> usize {
        lock(&self.inflight).as_ref().map_or(0, FlushBatch::len)
    }

    fn enqueue_record(&self, record: Arc<DirtyRecord<K, V>>) {
        let mut overflow = lock(&self.overflow);
        if !overflow.is_empty() {
            overflow.push_back(record);
            return;
        }
        drop(overflow);

        if let Err(PendingItem::One(record)) = self.queue.try_send_item(PendingItem::One(record)) {
            lock(&self.overflow).push_back(record);
        }
    }

    fn enqueue_batch(&self, records: Vec<Arc<DirtyRecord<K, V>>>) {
        let mut overflow = lock(&self.overflow);
        if !overflow.is_empty() {
            overflow.extend(records);
            return;
        }
        drop(overflow);

        if let Err(PendingItem::BatchSlice(records)) = self
            .queue
            .try_send_item(PendingItem::BatchSlice(records.into_boxed_slice()))
        {
            lock(&self.overflow).extend(records.into_vec());
        }
    }
}

impl<K, V> DirtyMode<K, V> for OrderedFifoDirty<K, V> {
    type VisibleDirty = Arc<DirtyRecord<K, V>>;

    fn new(capacity: usize) -> Self {
        Self {
            next_id: CachePadded(AtomicU64::new(0)),
            pending_len: CachePadded(AtomicUsize::new(0)),
            queue: DirtyQueue::new(capacity),
            append_lock: new_mutex(()),
            staged: new_mutex(VecDeque::new()),
            overflow: new_mutex(VecDeque::new()),
            inflight: new_mutex(None),
            #[cfg(test)]
            pending_shadow: new_mutex(VecDeque::new()),
        }
    }

    fn append(&self, key: K, value: V) -> Self::VisibleDirty {
        let _guard = lock(&self.append_lock);
        let id = self.next_id.0.fetch_add(1, Ordering::Relaxed);
        let record = Arc::new(DirtyRecord { id, key, value });
        self.enqueue_record(record.clone());
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
        self.enqueue_batch(out.clone());
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
                let mut overflow = lock(&self.overflow);
                while entries.len() < limit {
                    let Some(record) = overflow.pop_front() else {
                        break;
                    };
                    self.pending_len.0.fetch_sub(1, Ordering::Relaxed);
                    #[cfg(test)]
                    {
                        let _ = lock(&self.pending_shadow).pop_front();
                    }
                    entries.push(record);
                }
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
                PendingItem::ForceFlush => {}
            }
        }

        let batch = FlushBatch::new(entries);
        if !batch.is_empty() {
            *inflight = Some(batch.clone());
        }
        batch
    }

    fn wait_for_flush_work(&self, limit: usize) -> FlushWork<K, V> {
        let batch = self.flush_batch(limit);
        if !batch.is_empty() {
            return FlushWork::Batch(batch);
        }

        loop {
            let Some(item) = self.queue.recv_item() else {
                return FlushWork::ForceFlush;
            };

            match item {
                PendingItem::ForceFlush => return FlushWork::ForceFlush,
                PendingItem::One(record) => {
                    lock(&self.staged).push_back(record);
                }
                PendingItem::BatchSlice(records) => {
                    lock(&self.staged).extend(records.into_vec());
                }
            }

            let batch = self.flush_batch(limit);
            if !batch.is_empty() {
                return FlushWork::Batch(batch);
            }
        }
    }

    fn signal_force_flush(&self) {
        self.queue.send_force_flush();
    }

    fn mark_flushed(&self, batch: &FlushBatch<K, V>) -> usize {
        let mut inflight = lock(&self.inflight);
        let Some(current) = &*inflight else {
            return 0;
        };
        if !current.ptr_eq(batch)
            && (current.len() != batch.len() || current.last_id() != batch.last_id())
        {
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
    fn inflight_records(&self) -> Vec<Self::VisibleDirty>
    where
        K: Clone,
        V: Clone,
    {
        lock(&self.inflight)
            .as_ref()
            .map(FlushBatch::cloned_arc_entries)
            .unwrap_or_default()
    }

    #[cfg(any(test, feature = "loom"))]
    fn next_write(&self) -> WriteId {
        self.next_id.0.load(Ordering::Relaxed)
    }
}
