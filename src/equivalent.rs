use std::borrow::Borrow;

pub trait Equivalent<K: ?Sized> {
    fn equivalent(&self, key: &K) -> bool;
}

impl<Q, K> Equivalent<K> for Q
where
    Q: Eq + ?Sized,
    K: Borrow<Q> + ?Sized,
{
    fn equivalent(&self, key: &K) -> bool {
        self == key.borrow()
    }
}
