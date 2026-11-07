//! Category 4 — Size assertion.
//!
//! `size_of::<KeyBytes>()` MUST equal `size_of::<bytes::Bytes>()` on
//! this target — both are 32 bytes with `bytes` 1.11. See the module
//! docs of [`crate::key_bytes`] for why `INLINE_CAP` is 23 rather than
//! the plan doc's nominal 30.

use super::super::KeyBytes;
use bytes::Bytes;
use std::mem::size_of;

#[test]
fn keybytes_size_equals_bytes_size() {
    assert_eq!(size_of::<KeyBytes>(), size_of::<Bytes>());
}

#[test]
fn keybytes_size_is_32_bytes() {
    // The hard gate the plan doc requires — pin the concrete number so
    // a future regression in the layout (e.g. growing INLINE_CAP past
    // the safe limit, or adding a field) trips CI here, loudly.
    assert_eq!(
        size_of::<KeyBytes>(),
        32,
        "KeyBytes must be 32 bytes on this target"
    );
}
