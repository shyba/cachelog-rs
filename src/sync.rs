#[cfg(not(feature = "loom"))]
pub(crate) use std::sync::Arc;
#[cfg(not(feature = "loom"))]
pub(crate) use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

#[cfg(feature = "loom")]
pub(crate) use loom::sync::Arc;
#[cfg(feature = "loom")]
pub(crate) use loom::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

#[cfg(not(feature = "loom"))]
mod shared_arc_impl {
    use arc_swap::{ArcSwap, ArcSwapOption};

    use crate::sync::Arc;

    #[allow(dead_code)]
    pub(crate) struct SharedArc<T> {
        inner: ArcSwap<T>,
    }

    #[allow(dead_code)]
    impl<T> SharedArc<T> {
        pub(crate) fn new(value: Arc<T>) -> Self {
            Self {
                inner: ArcSwap::from(value),
            }
        }

        pub(crate) fn with<R>(&self, reader: impl FnOnce(&Arc<T>) -> R) -> R {
            let guard = self.inner.load();
            reader(&guard)
        }

        pub(crate) fn load(&self) -> Arc<T> {
            self.inner.load_full()
        }

        pub(crate) fn load_full(&self) -> Arc<T> {
            self.inner.load_full()
        }

        pub(crate) fn store(&self, value: Arc<T>) {
            self.inner.store(value);
        }

        pub(crate) fn swap(&self, value: Arc<T>) -> Arc<T> {
            self.inner.swap(value)
        }
    }

    #[allow(dead_code)]
    pub(crate) struct SharedOptionArc<T> {
        inner: ArcSwapOption<T>,
    }

    #[allow(dead_code)]
    impl<T> SharedOptionArc<T> {
        pub(crate) fn new(value: Option<Arc<T>>) -> Self {
            Self {
                inner: ArcSwapOption::from(value),
            }
        }

        pub(crate) fn empty() -> Self {
            Self::new(None)
        }

        pub(crate) fn with<R>(&self, reader: impl FnOnce(Option<&Arc<T>>) -> R) -> R {
            let guard = self.inner.load();
            reader(guard.as_ref())
        }

        pub(crate) fn load(&self) -> Option<Arc<T>> {
            self.inner.load_full()
        }

        pub(crate) fn load_full(&self) -> Option<Arc<T>> {
            self.inner.load_full()
        }

        pub(crate) fn store(&self, value: Option<Arc<T>>) {
            self.inner.store(value);
        }

        pub(crate) fn swap(&self, value: Option<Arc<T>>) -> Option<Arc<T>> {
            self.inner.swap(value)
        }
    }
}

#[cfg(feature = "loom")]
mod shared_arc_impl {
    use std::mem;

    use crate::sync::Arc;

    #[allow(dead_code)]
    pub(crate) struct SharedArc<T> {
        inner: loom::sync::Mutex<Arc<T>>,
    }

    #[allow(dead_code)]
    impl<T> SharedArc<T> {
        pub(crate) fn new(value: Arc<T>) -> Self {
            Self {
                inner: loom::sync::Mutex::new(value),
            }
        }

        pub(crate) fn with<R>(&self, reader: impl FnOnce(&Arc<T>) -> R) -> R {
            let guard = self.inner.lock().expect("poisoned");
            reader(&guard)
        }

        pub(crate) fn load(&self) -> Arc<T> {
            self.inner.lock().expect("poisoned").clone()
        }

        pub(crate) fn load_full(&self) -> Arc<T> {
            self.load()
        }

        pub(crate) fn store(&self, value: Arc<T>) {
            *self.inner.lock().expect("poisoned") = value;
        }

        pub(crate) fn swap(&self, value: Arc<T>) -> Arc<T> {
            let mut guard = self.inner.lock().expect("poisoned");
            mem::replace(&mut *guard, value)
        }
    }

    #[allow(dead_code)]
    pub(crate) struct SharedOptionArc<T> {
        inner: loom::sync::Mutex<Option<Arc<T>>>,
    }

    #[allow(dead_code)]
    impl<T> SharedOptionArc<T> {
        pub(crate) fn new(value: Option<Arc<T>>) -> Self {
            Self {
                inner: loom::sync::Mutex::new(value),
            }
        }

        pub(crate) fn empty() -> Self {
            Self::new(None)
        }

        pub(crate) fn with<R>(&self, reader: impl FnOnce(Option<&Arc<T>>) -> R) -> R {
            let guard = self.inner.lock().expect("poisoned");
            reader(guard.as_ref())
        }

        pub(crate) fn load(&self) -> Option<Arc<T>> {
            self.inner.lock().expect("poisoned").clone()
        }

        pub(crate) fn load_full(&self) -> Option<Arc<T>> {
            self.load()
        }

        pub(crate) fn store(&self, value: Option<Arc<T>>) {
            *self.inner.lock().expect("poisoned") = value;
        }

        pub(crate) fn swap(&self, value: Option<Arc<T>>) -> Option<Arc<T>> {
            let mut guard = self.inner.lock().expect("poisoned");
            mem::replace(&mut *guard, value)
        }
    }
}

#[allow(unused_imports)]
pub(crate) use shared_arc_impl::{SharedArc, SharedOptionArc};

#[cfg(not(feature = "loom"))]
mod mutex_impl {
    pub(crate) type Mutex<T> = parking_lot::Mutex<T>;

    pub(crate) fn new_mutex<T>(val: T) -> Mutex<T> {
        parking_lot::Mutex::new(val)
    }

    pub(crate) fn lock<T>(m: &Mutex<T>) -> parking_lot::MutexGuard<'_, T> {
        m.lock()
    }
}

#[cfg(feature = "loom")]
mod mutex_impl {
    pub(crate) type Mutex<T> = loom::sync::Mutex<T>;

    pub(crate) fn new_mutex<T>(val: T) -> Mutex<T> {
        loom::sync::Mutex::new(val)
    }

    pub(crate) fn lock<T>(m: &Mutex<T>) -> loom::sync::MutexGuard<'_, T> {
        m.lock().expect("poisoned")
    }
}

pub(crate) use mutex_impl::{Mutex, lock, new_mutex};

#[cfg(not(feature = "loom"))]
mod queue_impl {
    use crossbeam_queue::ArrayQueue;

    pub(crate) struct BoundedQueue<T> {
        inner: ArrayQueue<T>,
    }

    impl<T> BoundedQueue<T> {
        pub fn new(capacity: usize) -> Self {
            Self {
                inner: ArrayQueue::new(capacity.max(1)),
            }
        }

        pub fn push(&self, val: T) -> Result<(), T> {
            self.inner.push(val)
        }

        pub fn pop(&self) -> Option<T> {
            self.inner.pop()
        }
    }
}

#[cfg(feature = "loom")]
mod queue_impl {
    use std::collections::VecDeque;

    pub(crate) struct BoundedQueue<T> {
        inner: loom::sync::Mutex<VecDeque<T>>,
        capacity: usize,
    }

    impl<T> BoundedQueue<T> {
        pub fn new(capacity: usize) -> Self {
            Self {
                inner: loom::sync::Mutex::new(VecDeque::with_capacity(capacity.max(1))),
                capacity: capacity.max(1),
            }
        }

        pub fn push(&self, val: T) -> Result<(), T> {
            let mut guard = self.inner.lock().expect("poisoned");
            if guard.len() >= self.capacity {
                return Err(val);
            }
            guard.push_back(val);
            Ok(())
        }

        pub fn pop(&self) -> Option<T> {
            self.inner.lock().expect("poisoned").pop_front()
        }
    }
}

pub(crate) use queue_impl::BoundedQueue;

#[cfg(not(feature = "loom"))]
#[repr(align(64))]
pub(crate) struct CachePadded<T>(pub T);
