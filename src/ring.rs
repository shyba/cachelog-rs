use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};

#[repr(align(64))]
struct CachePadded<T>(T);

pub const PAYLOAD_WORDS: usize = 512;
pub type Payload = [u64; PAYLOAD_WORDS];

/// Compile-time read strategy marker.
pub trait ReadMode {
    const CHECKED: bool;
}

/// Fast path: one base check, one tag check.
pub enum Direct {}

/// Extra validation path: re-checks tag/base after reading.
pub enum Checked {}

impl ReadMode for Direct {
    const CHECKED: bool = false;
}

impl ReadMode for Checked {
    const CHECKED: bool = true;
}

#[repr(align(64))]
struct Slot {
    /// Generation tag: `id + 1` when occupied, `0` when empty.
    tag: AtomicU64,
    /// Full 4KiB payload (512 * u64), atomically read/written per word.
    value: [AtomicU64; PAYLOAD_WORDS],
}

impl Slot {
    fn new() -> Self {
        Self {
            tag: AtomicU64::new(0),
            value: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

/// Monotonic ring over `u64` payloads.
///
/// - `push` allocates a unique id (`top.fetch_add`)
/// - `read` resolves id -> slot, checks generation tag, then reads value
/// - `pop`/`pop_n` advances `base` and conditionally clears matching slots
#[repr(align(64))]
pub struct MonotonicRing<M: ReadMode = Direct> {
    mask: u64,
    capacity: u64,
    top: CachePadded<AtomicU64>,
    base: CachePadded<AtomicU64>,
    slots: Box<[Slot]>,
    _mode: PhantomData<M>,
}

impl<M: ReadMode> MonotonicRing<M> {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "capacity must be > 0");
        let cap_pow2 = capacity.next_power_of_two();
        let mut slots = Vec::with_capacity(cap_pow2);
        slots.resize_with(cap_pow2, Slot::new);
        Self {
            mask: (cap_pow2 as u64) - 1,
            capacity: cap_pow2 as u64,
            top: CachePadded(AtomicU64::new(0)),
            base: CachePadded(AtomicU64::new(0)),
            slots: slots.into_boxed_slice(),
            _mode: PhantomData,
        }
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity as usize
    }

    #[inline]
    pub fn top(&self) -> u64 {
        self.top.0.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn base(&self) -> u64 {
        self.base.0.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn push(&self, value: Payload) -> u64 {
        let id = self.top.0.fetch_add(1, Ordering::Relaxed);
        let slot = &self.slots[(id & self.mask) as usize];
        // Invalidate first so readers never accept an in-progress rewrite.
        slot.tag.store(0, Ordering::Release);
        for (dst, src) in slot.value.iter().zip(value) {
            dst.store(src, Ordering::Relaxed);
        }
        slot.tag.store(id.wrapping_add(1), Ordering::Release);
        id
    }

    #[inline]
    pub fn read<R>(&self, id: u64, f: impl FnOnce(&Payload) -> R) -> Option<R> {
        let base0 = self.base.0.load(Ordering::Relaxed);
        if id < base0 {
            return None;
        }

        let slot = &self.slots[(id & self.mask) as usize];
        let expected = id.wrapping_add(1);
        let tag0 = slot.tag.load(Ordering::Acquire);
        if tag0 != expected {
            return None;
        }

        let value = std::array::from_fn(|i| slot.value[i].load(Ordering::Acquire));

        if M::CHECKED {
            let tag1 = slot.tag.load(Ordering::Acquire);
            let base1 = self.base.0.load(Ordering::Relaxed);
            if tag1 != expected || id < base1 {
                return None;
            }
        }

        Some(f(&value))
    }

    #[inline]
    pub fn pop(&self) -> Option<(u64, u64)> {
        let id = self.claim_pop_id()?;
        Some((id, self.clear_if_matches(id)?))
    }

    /// Pop up to `n` ids by advancing base once, then conditionally clearing
    /// matching slots. Returns how many ids were actually popped.
    #[inline]
    pub fn pop_n(&self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        let (start, count) = self.claim_pop_range(n);
        for id in start..start + count {
            let _ = self.clear_if_matches(id);
        }
        count
    }

    #[inline]
    fn claim_pop_id(&self) -> Option<u64> {
        loop {
            let b = self.base.0.load(Ordering::Relaxed);
            let t = self.top.0.load(Ordering::Acquire);
            if b >= t {
                return None;
            }
            if self
                .base
                .0
                .compare_exchange(b, b + 1, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                return Some(b);
            }
        }
    }

    #[inline]
    fn claim_pop_range(&self, n: u64) -> (u64, u64) {
        loop {
            let b = self.base.0.load(Ordering::Relaxed);
            let t = self.top.0.load(Ordering::Acquire);
            if b >= t {
                return (b, 0);
            }
            let avail = t - b;
            let take = avail.min(n);
            if self
                .base
                .0
                .compare_exchange(b, b + take, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                return (b, take);
            }
        }
    }

    #[inline]
    fn clear_if_matches(&self, id: u64) -> Option<u64> {
        let slot = &self.slots[(id & self.mask) as usize];
        let expected = id.wrapping_add(1);
        if slot
            .tag
            .compare_exchange(expected, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            Some(slot.value[0].load(Ordering::Acquire))
        } else {
            None
        }
    }
}
