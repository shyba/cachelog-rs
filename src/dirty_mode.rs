use crate::entry::FlushBatch;

pub(crate) enum FlushWork<K, V> {
    Batch(FlushBatch<K, V>),
    ForceFlush,
}

#[cfg(feature = "loom")]
mod imp {
    use std::collections::VecDeque;

    #[cfg(any(test, feature = "dev-tools"))]
    use bytes::Bytes;

    #[cfg(any(test, feature = "dev-tools"))]
    use crate::bytes_pooling::bytes_from_borrowed;
    use crate::dirty_mode::FlushWork;
    use crate::entry::{DirtyRecord, FlushBatch, WriteId};
    use crate::sync::{Arc, Mutex, lock, new_mutex};

    #[cfg(any(test, feature = "dev-tools"))]
    type BorrowedArcVecDirtyRecord = Arc<DirtyRecord<Vec<u8>, Arc<Vec<u8>>>>;

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

    struct DirtyLog<K, V> {
        next_id: WriteId,
        entries: VecDeque<Arc<DirtyRecord<K, V>>>,
        inflight: Option<FlushBatch<K, V>>,
        force_flush_pending: bool,
    }

    impl<K, V> DirtyLog<K, V> {
        fn with_capacity(capacity: usize) -> Self {
            Self {
                next_id: 0,
                entries: VecDeque::with_capacity(capacity.max(1)),
                inflight: None,
                force_flush_pending: false,
            }
        }
    }

    pub(crate) struct OrderedFifoDirty<K, V> {
        inner: Mutex<DirtyLog<K, V>>,
    }

    impl<K, V> DirtyMode<K, V> for OrderedFifoDirty<K, V> {
        type VisibleDirty = Arc<DirtyRecord<K, V>>;

        fn new(capacity: usize) -> Self {
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

        fn wait_for_flush_work(&self, limit: usize) -> FlushWork<K, V> {
            let mut inner = lock(&self.inner);
            if let Some(batch) = &inner.inflight {
                return FlushWork::Batch(batch.clone());
            }
            if inner.force_flush_pending {
                inner.force_flush_pending = false;
                return FlushWork::ForceFlush;
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
                return FlushWork::Batch(batch);
            }
            FlushWork::Batch(batch)
        }

        fn signal_force_flush(&self) {
            let mut inner = lock(&self.inner);
            inner.force_flush_pending = true;
        }

        fn mark_flushed(&self, batch: &FlushBatch<K, V>) -> usize {
            let mut inner = lock(&self.inner);
            let Some(inflight) = &inner.inflight else {
                return 0;
            };
            if !inflight.ptr_eq(batch)
                && (inflight.len() != batch.len() || inflight.last_id() != batch.last_id())
            {
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
        fn inflight_records(&self) -> Vec<Self::VisibleDirty>
        where
            K: Clone,
            V: Clone,
        {
            lock(&self.inner)
                .inflight
                .as_ref()
                .map(FlushBatch::cloned_arc_entries)
                .unwrap_or_default()
        }

        #[cfg(any(test, feature = "loom"))]
        fn next_write(&self) -> WriteId {
            lock(&self.inner).next_id
        }
    }

    impl<K, V> OrderedFifoDirty<K, V> {
        pub(crate) fn pending_len(&self) -> usize {
            lock(&self.inner).entries.len()
        }

        pub(crate) fn inflight_len(&self) -> usize {
            lock(&self.inner)
                .inflight
                .as_ref()
                .map_or(0, FlushBatch::len)
        }
    }

    impl OrderedFifoDirty<Vec<u8>, Vec<u8>> {
        pub(crate) fn append_batch_borrowed<'a, I>(
            &self,
            entries: I,
        ) -> Vec<Arc<DirtyRecord<Vec<u8>, Vec<u8>>>>
        where
            I: IntoIterator<Item = (&'a [u8], &'a [u8])>,
        {
            let iter = entries.into_iter();
            let (lower, upper) = iter.size_hint();
            let mut inner = lock(&self.inner);
            let mut out = Vec::with_capacity(upper.unwrap_or(lower));
            for (key, value) in iter {
                let id = inner.next_id;
                inner.next_id = inner.next_id.wrapping_add(1);
                let record = Arc::new(DirtyRecord {
                    id,
                    key: key.to_vec(),
                    value: value.to_vec(),
                });
                inner.entries.push_back(record.clone());
                out.push(record);
            }
            out
        }
    }

    #[cfg(any(test, feature = "dev-tools"))]
    impl OrderedFifoDirty<Vec<u8>, Arc<Vec<u8>>> {
        pub(crate) fn append_batch_borrowed<'a, I>(
            &self,
            entries: I,
        ) -> Vec<BorrowedArcVecDirtyRecord>
        where
            I: IntoIterator<Item = (&'a [u8], &'a [u8])>,
        {
            let iter = entries.into_iter();
            let (lower, upper) = iter.size_hint();
            let mut inner = lock(&self.inner);
            let mut out = Vec::with_capacity(upper.unwrap_or(lower));
            for (key, value) in iter {
                let id = inner.next_id;
                inner.next_id = inner.next_id.wrapping_add(1);
                let record = Arc::new(DirtyRecord {
                    id,
                    key: key.to_vec(),
                    value: Arc::new(value.to_vec()),
                });
                inner.entries.push_back(record.clone());
                out.push(record);
            }
            out
        }
    }
    #[cfg(any(test, feature = "dev-tools"))]
    impl OrderedFifoDirty<Vec<u8>, Bytes> {
        pub(crate) fn append_batch_borrowed<'a, I>(
            &self,
            entries: I,
        ) -> Vec<Arc<DirtyRecord<Vec<u8>, Bytes>>>
        where
            I: IntoIterator<Item = (&'a [u8], &'a [u8])>,
        {
            let iter = entries.into_iter();
            let (lower, upper) = iter.size_hint();
            let mut inner = lock(&self.inner);
            let mut out = Vec::with_capacity(upper.unwrap_or(lower));
            for (key, value) in iter {
                let id = inner.next_id;
                inner.next_id = inner.next_id.wrapping_add(1);
                let record = Arc::new(DirtyRecord {
                    id,
                    key: key.to_vec(),
                    value: bytes_from_borrowed(value),
                });
                inner.entries.push_back(record.clone());
                out.push(record);
            }
            out
        }
    }
}

#[cfg(not(feature = "loom"))]
mod imp {
    use std::collections::VecDeque;

    #[cfg(any(test, feature = "dev-tools"))]
    use bytes::Bytes;
    use crossbeam_channel as cbch;

    #[cfg(any(test, feature = "dev-tools"))]
    use crate::bytes_pooling::bytes_from_borrowed;
    use crate::dirty_mode::FlushWork;
    #[cfg(any(test, feature = "loom"))]
    use crate::entry::WriteId;
    use crate::entry::{DirtyRecord, FlushBatch};
    use crate::sync::{Arc, AtomicU64, AtomicUsize, CachePadded, Mutex, Ordering, lock, new_mutex};

    #[cfg(any(test, feature = "dev-tools"))]
    type BorrowedArcVecDirtyRecord = Arc<DirtyRecord<Vec<u8>, Arc<Vec<u8>>>>;
    #[cfg(any(test, feature = "dev-tools"))]
    type BorrowedArcSliceDirtyRecord = Arc<DirtyRecord<Vec<u8>, Arc<[u8]>>>;

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

    enum PendingItem<K, V> {
        One(Arc<DirtyRecord<K, V>>),
        BatchSlice(Box<[Arc<DirtyRecord<K, V>>]>),
        ForceFlush,
    }

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

            if let Err(PendingItem::One(record)) =
                self.queue.try_send_item(PendingItem::One(record))
            {
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

    impl OrderedFifoDirty<Vec<u8>, Vec<u8>> {
        pub(crate) fn append_batch_borrowed<'a, I>(
            &self,
            entries: I,
        ) -> Vec<Arc<DirtyRecord<Vec<u8>, Vec<u8>>>>
        where
            I: IntoIterator<Item = (&'a [u8], &'a [u8])>,
        {
            let entries: Vec<_> = entries.into_iter().collect();
            let len = entries.len();
            if len == 0 {
                return Vec::new();
            }
            let _guard = lock(&self.append_lock);
            let base = self.next_id.0.fetch_add(len as u64, Ordering::Relaxed);
            let mut out = Vec::with_capacity(len);
            for (offset, (key, value)) in entries.into_iter().enumerate() {
                let record = Arc::new(DirtyRecord {
                    id: base.wrapping_add(offset as u64),
                    key: key.to_vec(),
                    value: value.to_vec(),
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
    }

    #[cfg(any(test, feature = "dev-tools"))]
    impl OrderedFifoDirty<Vec<u8>, Arc<Vec<u8>>> {
        pub(crate) fn append_batch_borrowed<'a, I>(
            &self,
            entries: I,
        ) -> Vec<BorrowedArcVecDirtyRecord>
        where
            I: IntoIterator<Item = (&'a [u8], &'a [u8])>,
        {
            let entries: Vec<_> = entries.into_iter().collect();
            let len = entries.len();
            if len == 0 {
                return Vec::new();
            }
            let _guard = lock(&self.append_lock);
            let base = self.next_id.0.fetch_add(len as u64, Ordering::Relaxed);
            let mut out = Vec::with_capacity(len);
            for (offset, (key, value)) in entries.into_iter().enumerate() {
                let record = Arc::new(DirtyRecord {
                    id: base.wrapping_add(offset as u64),
                    key: key.to_vec(),
                    value: Arc::new(value.to_vec()),
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
    }

    #[cfg(any(test, feature = "dev-tools"))]
    impl OrderedFifoDirty<Vec<u8>, Arc<[u8]>> {
        pub(crate) fn append_batch_borrowed<'a, I>(
            &self,
            entries: I,
        ) -> Vec<BorrowedArcSliceDirtyRecord>
        where
            I: IntoIterator<Item = (&'a [u8], &'a [u8])>,
        {
            let entries: Vec<_> = entries.into_iter().collect();
            let len = entries.len();
            if len == 0 {
                return Vec::new();
            }
            let _guard = lock(&self.append_lock);
            let base = self.next_id.0.fetch_add(len as u64, Ordering::Relaxed);
            let mut out = Vec::with_capacity(len);
            for (offset, (key, value)) in entries.into_iter().enumerate() {
                let record = Arc::new(DirtyRecord {
                    id: base.wrapping_add(offset as u64),
                    key: key.to_vec(),
                    value: Arc::from(value.to_vec().into_boxed_slice()),
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
    }

    #[cfg(any(test, feature = "dev-tools"))]
    impl OrderedFifoDirty<Vec<u8>, Bytes> {
        pub(crate) fn append_batch_borrowed<'a, I>(
            &self,
            entries: I,
        ) -> Vec<Arc<DirtyRecord<Vec<u8>, Bytes>>>
        where
            I: IntoIterator<Item = (&'a [u8], &'a [u8])>,
        {
            let entries: Vec<_> = entries.into_iter().collect();
            let len = entries.len();
            if len == 0 {
                return Vec::new();
            }
            let _guard = lock(&self.append_lock);
            let base = self.next_id.0.fetch_add(len as u64, Ordering::Relaxed);
            let mut out = Vec::with_capacity(len);
            for (offset, (key, value)) in entries.into_iter().enumerate() {
                let record = Arc::new(DirtyRecord {
                    id: base.wrapping_add(offset as u64),
                    key: key.to_vec(),
                    value: bytes_from_borrowed(value),
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
    }
}

pub(crate) use imp::{DirtyMode, OrderedFifoDirty};
