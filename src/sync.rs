#[cfg(not(feature = "loom"))]
pub(crate) use std::sync::Arc;
#[cfg(not(feature = "loom"))]
pub(crate) use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

#[cfg(feature = "loom")]
pub(crate) use loom::sync::Arc;
#[cfg(feature = "loom")]
pub(crate) use loom::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

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

#[repr(align(64))]
pub(crate) struct CachePadded<T>(pub T);
