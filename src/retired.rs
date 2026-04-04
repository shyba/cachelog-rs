use crate::entry::{Entry, EntryState};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetiredEntry<K, V> {
    pub key: K,
    pub value: V,
    pub state: EntryState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetiredEntryRef<'a, K, V> {
    pub key: &'a K,
    pub value: &'a V,
    pub state: EntryState,
}

pub struct RetiredGeneration<K, V> {
    pub(crate) entries: Vec<Entry<K, V>>,
}

impl<K, V> RetiredGeneration<K, V> {
    pub(crate) fn new(entries: Vec<Entry<K, V>>) -> Self {
        Self { entries }
    }

    pub(crate) fn into_entries(self) -> Vec<Entry<K, V>> {
        self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = RetiredEntryRef<'_, K, V>> {
        self.entries.iter().map(RetiredEntryRef::from)
    }

    pub fn iter_dirty(&self) -> impl Iterator<Item = RetiredEntryRef<'_, K, V>> {
        self.entries
            .iter()
            .filter(|entry| entry.state == EntryState::Dirty)
            .map(RetiredEntryRef::from)
    }

    pub fn iter_clean(&self) -> impl Iterator<Item = RetiredEntryRef<'_, K, V>> {
        self.entries
            .iter()
            .filter(|entry| entry.state == EntryState::Clean)
            .map(RetiredEntryRef::from)
    }

    pub fn drain_dirty(&mut self) -> Vec<RetiredEntry<K, V>> {
        self.drain_matching(EntryState::Dirty)
    }

    pub fn drain_clean(&mut self) -> Vec<RetiredEntry<K, V>> {
        self.drain_matching(EntryState::Clean)
    }

    pub fn process_retired<FD, FC, ED, EC>(
        self,
        flush_dirty: FD,
        drain_clean: FC,
    ) -> Result<(), Self>
    where
        FD: FnOnce(&Self) -> Result<(), ED>,
        FC: FnOnce(&Self) -> Result<(), EC>,
    {
        if drain_clean(&self).is_err() {
            return Err(self);
        }
        if flush_dirty(&self).is_err() {
            return Err(self);
        }
        Ok(())
    }

    fn drain_matching(&mut self, state: EntryState) -> Vec<RetiredEntry<K, V>> {
        let entries = std::mem::take(&mut self.entries);
        let mut drained = Vec::new();
        let mut retained = Vec::new();

        for entry in entries {
            if entry.state == state {
                drained.push(entry.into());
            } else {
                retained.push(entry);
            }
        }

        self.entries = retained;
        drained
    }
}

impl<'a, K, V> From<&'a Entry<K, V>> for RetiredEntryRef<'a, K, V> {
    fn from(entry: &'a Entry<K, V>) -> Self {
        Self {
            key: &entry.key,
            value: &entry.value,
            state: entry.state,
        }
    }
}

impl<K, V> From<Entry<K, V>> for RetiredEntry<K, V> {
    fn from(entry: Entry<K, V>) -> Self {
        Self {
            key: entry.key,
            value: entry.value,
            state: entry.state,
        }
    }
}
