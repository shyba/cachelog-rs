//! Visibility helpers.

use super::state::{VisibleRef, WriteId};

pub fn contains_visible_dirty(items: &[VisibleRef], needle: WriteId) -> bool {
    let mut index = 0;
    while index < items.len() {
        if items[index] == VisibleRef::Dirty(needle) {
            return true;
        }
        index += 1;
    }
    false
}
