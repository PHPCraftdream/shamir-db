//! Layer a (low) reaching up through a crate-root re-export.

use crate::inner::Item;

pub struct Thing;

pub fn touch() -> Item {
    Item
}
