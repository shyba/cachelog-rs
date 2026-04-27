#[cfg(not(feature = "loom"))]
mod imp {
    use bytes::{Bytes, BytesMut};
    use std::cell::RefCell;

    const INITIAL_CAPACITY: usize = 64 * 1024;
    const MAX_SPARE_CAPACITY: usize = 1 << 20;
    const SMALL_COPY_THRESHOLD: usize = 256;

    thread_local! {
        static BYTE_POOL: RefCell<BytesMut> = RefCell::new(BytesMut::with_capacity(INITIAL_CAPACITY));
    }

    pub(crate) fn bytes_from_borrowed(src: &[u8]) -> Bytes {
        if src.len() <= SMALL_COPY_THRESHOLD {
            return Bytes::copy_from_slice(src);
        }

        BYTE_POOL.with(|pool| {
            let mut pool = pool.borrow_mut();
            let _ = pool.try_reclaim(src.len());
            pool.extend_from_slice(src);
            let bytes = pool.split().freeze();

            if pool.capacity() > MAX_SPARE_CAPACITY {
                *pool = BytesMut::with_capacity(MAX_SPARE_CAPACITY);
            }

            bytes
        })
    }
}

#[cfg(feature = "loom")]
mod imp {
    use bytes::Bytes;

    pub(crate) fn bytes_from_borrowed(src: &[u8]) -> Bytes {
        Bytes::copy_from_slice(src)
    }
}

pub(crate) use imp::bytes_from_borrowed;
