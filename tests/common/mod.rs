#![allow(dead_code)]

use std::borrow::Borrow;
use std::collections::hash_map::DefaultHasher;
use std::hash::{BuildHasher, Hasher};

use cachelog::{CacheLogMap, EntryState, low_level::VisibleRef};

pub fn read_pair<K, Q, V>(map: &CacheLogMap<K, V>, key: &Q) -> Option<(V, EntryState)>
where
    K: Borrow<Q> + Clone + Eq + std::hash::Hash,
    Q: Eq + std::hash::Hash + ?Sized,
    V: Copy,
{
    map.read(key, |_, value, state| (*value, state))
}

pub fn read_triplet<K, Q, V>(
    map: &CacheLogMap<K, V>,
    key: &Q,
) -> Option<(V, EntryState, VisibleRef)>
where
    K: Borrow<Q> + Clone + Eq + std::hash::Hash,
    Q: Eq + std::hash::Hash + ?Sized,
    V: Copy,
{
    map.low_level()
        .read_full(key, |_, value, state, visible| (*value, state, visible))
}

#[derive(Clone, Default)]
pub struct PrefixOrderBuildHasher;

#[derive(Default)]
pub struct PrefixOrderHasher {
    bytes: Vec<u8>,
}

impl Hasher for PrefixOrderHasher {
    fn finish(&self) -> u64 {
        match self.bytes.iter().rev().find(|&&b| b.is_ascii_lowercase()) {
            Some(b'b') => 0,
            Some(b'c') => 1,
            Some(b'a') => 2,
            Some(other) => u64::from(*other),
            None => DefaultHasher::new().finish(),
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }
}

impl BuildHasher for PrefixOrderBuildHasher {
    type Hasher = PrefixOrderHasher;

    fn build_hasher(&self) -> Self::Hasher {
        PrefixOrderHasher::default()
    }
}
