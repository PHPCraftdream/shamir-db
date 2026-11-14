//! Layer a (low) reaching up through a grouped use.

use crate::b::{Thing1, Thing2};

pub struct Thing3;

pub fn touch() -> (Thing1, Thing2) {
    (Thing1, Thing2)
}
