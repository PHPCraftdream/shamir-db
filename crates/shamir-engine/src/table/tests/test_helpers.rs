//! Shared test utilities for table tests.
//!
//! Replaces the removed `shamir_types::codecs::transform::user_to_inner` with
//! a thin wrapper around the surviving `query_value_to_inner_with`, tracking
//! newly-interned keys so callers can persist them via `InternerManager::save_new_keys`.

use std::cell::RefCell;

use shamir_types::codecs::interned::query_value_to_inner_with;
use shamir_types::codecs::CodecError;
use shamir_types::core::interner::{Interner, InternerKey, UserKey};
use shamir_types::types::value::{InnerValue, QueryValue};

/// Convert a `QueryValue` (string-keyed) to `InnerValue` (interned keys),
/// tracking which field names were newly interned.
///
/// Returns `(inner_value, new_keys)` where `new_keys` is the list of
/// `(InternerKey, UserKey)` pairs that were created during this call.
/// Pass `new_keys` to `InternerManager::save_new_keys` to persist them.
pub fn query_value_to_inner_tracked(
    qv: &QueryValue,
    interner: &Interner,
) -> Result<(InnerValue, Vec<(InternerKey, UserKey)>), CodecError> {
    let new_keys: RefCell<Vec<(InternerKey, UserKey)>> = RefCell::new(Vec::new());

    let intern_fn = |key: &str| -> Result<InternerKey, CodecError> {
        let ti = interner.touch_ind(key).map_err(|e| {
            CodecError::Decode(format!("Failed to intern key '{}': {}", key, e))
        })?;
        if ti.is_new() {
            new_keys
                .borrow_mut()
                .push((ti.key().clone(), UserKey::from_str(key)));
        }
        Ok(ti.into_key())
    };

    let inner = query_value_to_inner_with(qv, &intern_fn)?;
    Ok((inner, new_keys.into_inner()))
}
