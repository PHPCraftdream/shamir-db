//! Common utilities for interned codecs
//!
//! This module contains shared abstractions to avoid duplication between
//! the interned codec and MessagePack codec implementations.

use crate::codecs::CodecError;
use crate::core::interner::{Interner, InternerKey};

#[cfg(test)]
use std::cell::Cell;

#[cfg(test)]
thread_local! {
    static LAST_KEY_PTR: Cell<usize> = const { Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn last_intern_key_ptr() -> usize {
    LAST_KEY_PTR.with(Cell::get)
}

/// Helper function to intern a string key for map entries
///
/// This is used by both the interned codec and the MessagePack codec to intern
/// map keys during conversion from external format to InnerValue.
pub fn intern_string_key(interner: &Interner, key_str: &str) -> Result<InternerKey, CodecError> {
    #[cfg(test)]
    LAST_KEY_PTR.with(|cell| cell.set(key_str.as_ptr() as usize));

    interner
        .touch_ind(key_str)
        .map(|t| t.into_key())
        .map_err(CodecError::from)
}

/// Helper function to de-intern a key from InternedKey to String
///
/// This is used by both the interned codec and the MessagePack codec to resolve
/// interned keys back to their string representation.
pub fn deintern_key(interner: &Interner, interned_key: &InternerKey) -> Result<String, CodecError> {
    interner
        .with_str(interned_key, |s| s.to_string())
        .ok_or_else(|| CodecError::Decode(format!("Interned key not found: {:?}", interned_key)))
}
