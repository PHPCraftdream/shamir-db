//! Common utilities for interned codecs
//!
//! This module contains shared abstractions to avoid duplication between
//! the interned codec and MessagePack codec implementations.

use crate::codecs::CodecError;
use crate::core::interner::{Interner, InternerKey};

/// Helper function to intern a string key for map entries
///
/// This is used by both the interned codec and the MessagePack codec to intern
/// map keys during conversion from external format to InnerValue.
pub fn intern_string_key(interner: &Interner, key_str: &str) -> Result<InternerKey, CodecError> {
    interner
        .touch_ind(key_str)
        .map(|t| t.into_key())
        .map_err(|e| CodecError::Decode(format!("Failed to intern key '{}': {}", key_str, e)))
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
