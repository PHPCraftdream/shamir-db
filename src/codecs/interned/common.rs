//! Common utilities for interned codecs
//!
//! This module contains shared abstractions to avoid duplication between
//! JSON and MessagePack codec implementations.

use crate::codecs::CodecError;
use crate::core::interner::{InternerKey, Interner};

/// Helper function to intern a string key for map entries
///
/// This is used by both JSON and MessagePack codecs to intern
/// map keys during conversion from external format to InnerValue.
pub fn intern_string_key(interner: &Interner, key_str: &str) -> Result<InternerKey, CodecError> {
    Ok(interner
        .touch_ind(key_str)
        .map_err(|e| CodecError::Decode(format!("Failed to intern key '{}': {}", key_str, e)))?
        .key()
        .clone())
}

/// Helper function to de-intern a key from InternedKey to String
///
/// This is used by both JSON and MessagePack codecs to resolve
/// interned keys back to their string representation.
pub fn deintern_key(interner: &Interner, interned_key: &InternerKey) -> String {
    interner
        .get_str(interned_key)
        .expect("Interned key not found in interner")
        .as_ref()
        .to_string()
}
