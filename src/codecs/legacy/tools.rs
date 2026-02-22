#![allow(deprecated)]

//! DEPRECATED: Transform functionality for UserValue <-> InnerValue conversion.
//!
//! **This module is deprecated.** Use the newer codec-based approach in the parent
//! `codecs` module instead.

use crate::core::interner::{InternerKey, Interner, UserKey};
use crate::types::common::{new_map_wc, new_set_wc};
use crate::types::value::{InnerValue, UserValue, Value};

/// The result of a `user_to_inner` transformation.
///
/// **Deprecated.** Use newer codec-based approach instead.
///
/// This struct contains a resulting `InnerValue` and an optional collection
/// of any string keys that were newly interned during transformation.
#[deprecated(since = "0.1.0", note = "Use newer codec-based approach instead")]
#[derive(Debug)]
pub struct TransformResult {
    pub inner_value: InnerValue,
    pub new_keys: Option<Vec<(InternerKey, UserKey)>>,
}

impl TransformResult {
    /// Checks if any new keys were created during transformation.
    ///
    /// **Deprecated.** Use newer codec-based approach instead.
    /// Returns true if `new_keys` is `Some` and not empty.
    #[deprecated(since = "0.1.0", note = "Use newer codec-based approach instead")]
    pub fn has_new_keys(&self) -> bool {
        self.new_keys.as_ref().is_some_and(|v| !v.is_empty())
    }

    /// Consumes result and returns its constituent parts.
    ///
    /// **Deprecated.** Use newer codec-based approach instead.
    #[deprecated(since = "0.1.0", note = "Use newer codec-based approach instead")]
    pub fn into_parts(self) -> (InnerValue, Option<Vec<(InternerKey, UserKey)>>) {
        (self.inner_value, self.new_keys)
    }

    /// Consumes result and returns just the inner value.
    ///
    /// **Deprecated.** Use newer codec-based approach instead.
    #[deprecated(since = "0.1.0", note = "Use newer codec-based approach instead")]
    pub fn into_inner_value(self) -> InnerValue {
        self.inner_value
    }
}

fn user_to_inner_rec(
    value: &UserValue,
    interner: &Interner,
    new_keys: &mut Option<Vec<(InternerKey, UserKey)>>,
) -> InnerValue {
    match value {
        Value::Nil => Value::Nil,
        Value::Bool(b) => Value::Bool(*b),
        Value::Int(i) => Value::Int(*i),
        Value::F64(f) => Value::F64(*f),
        Value::Dec(d) => Value::Dec(*d),
        Value::Big(b) => Value::Big(b.clone()),
        Value::Str(s) => Value::Str(s.clone()),
        Value::Bin(b) => Value::Bin(b.clone()),
        Value::List(list) => {
            let inner_list = list
                .iter()
                .map(|v| user_to_inner_rec(v, interner, new_keys))
                .collect();
            Value::List(inner_list)
        }
        Value::Set(set) => {
            let mut inner_set = new_set_wc(set.len());
            for v in set {
                inner_set.insert(user_to_inner_rec(v, interner, new_keys));
            }
            Value::Set(inner_set)
        }
        Value::Map(map) => {
            let mut inner_map = new_map_wc(map.len());
            for (key, val) in map {
                let interned_key = interner.touch_ind(key).unwrap();
                if interned_key.is_new() {
                    new_keys
                        .get_or_insert_with(Vec::new)
                        .push((interned_key.key().clone(), UserKey::from_str(key)));
                }
                let inner_val = user_to_inner_rec(val, interner, new_keys);
                inner_map.insert(interned_key.key().clone(), inner_val);
            }
            Value::Map(inner_map)
        }
    }
}

/// Transforms a UserValue to an InnerValue, collecting newly interned keys.
///
/// **Deprecated.** Use newer codec-based approach instead.
///
/// This function is optimized to avoid heap allocations for the key collection
/// if no new keys are found.
#[deprecated(since = "0.1.0", note = "Use newer codec-based approach instead")]
pub fn user_to_inner(value: &UserValue, interner: &Interner) -> TransformResult {
    let mut new_keys: Option<Vec<(InternerKey, UserKey)>> = None;
    let inner_value = user_to_inner_rec(value, interner, &mut new_keys);
    TransformResult {
        inner_value,
        new_keys,
    }
}

/// Transforms an InnerValue to a UserValue, resolving interned keys.
///
/// **Deprecated.** Use newer codec-based approach instead.
#[deprecated(since = "0.1.0", note = "Use newer codec-based approach instead")]
pub fn inner_to_user(value: &InnerValue, interner: &Interner) -> UserValue {
    match value {
        Value::Nil => Value::Nil,
        Value::Bool(b) => Value::Bool(*b),
        Value::Int(i) => Value::Int(*i),
        Value::F64(f) => Value::F64(*f),
        Value::Dec(d) => Value::Dec(*d),
        Value::Big(b) => Value::Big(b.clone()),
        Value::Str(s) => Value::Str(s.clone()),
        Value::Bin(b) => Value::Bin(b.clone()),
        Value::List(list) => {
            let user_list = list.iter().map(|v| inner_to_user(v, interner)).collect();
            Value::List(user_list)
        }
        Value::Set(set) => {
            let mut user_set = new_set_wc(set.len());
            for v in set {
                user_set.insert(inner_to_user(v, interner));
            }
            Value::Set(user_set)
        }
        Value::Map(map) => {
            let mut user_map = new_map_wc(map.len());
            for (key_id, val) in map {
                let key = interner
                    .get_str(key_id)
                    .expect("Data corruption: interned key not found");
                let user_val = inner_to_user(val, interner);
                user_map.insert(key.as_str().to_string(), user_val);
            }
            Value::Map(user_map)
        }
    }
}
