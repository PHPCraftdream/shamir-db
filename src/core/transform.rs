#![allow(deprecated)]

use crate::core::interner::{Interner, InternedKey, UserKey};
use crate::types::common::{new_map_wc, new_set_wc};
use crate::types::value::{InnerValue, UserValue, Value};

/// The result of a `user_to_inner` transformation.
///
/// This struct contains the resulting `InnerValue` and an optional collection
/// of any string keys that were newly interned during the transformation.
#[derive(Debug)]
pub struct TransformResult {
    pub inner_value: InnerValue,
    pub new_keys: Option<Vec<(InternedKey, UserKey)>>,
}

impl TransformResult {
    /// Checks if any new keys were created during the transformation.
    /// Returns true if `new_keys` is `Some` and not empty.
    pub fn has_new_keys(&self) -> bool {
        self.new_keys.as_ref().is_some_and(|v| !v.is_empty())
    }

    /// Consumes the result and returns its constituent parts.
    pub fn into_parts(self) -> (InnerValue, Option<Vec<(InternedKey, UserKey)>>) {
        (self.inner_value, self.new_keys)
    }

    /// Consumes the result and returns just the inner value.
    pub fn into_inner_value(self) -> InnerValue {
        self.inner_value
    }
}

fn user_to_inner_rec(
    value: &UserValue,
    interner: &Interner,
    new_keys: &mut Option<Vec<(InternedKey, UserKey)>>,
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
/// This function is optimized to avoid heap allocations for the key collection
/// if no new keys are found.
pub fn user_to_inner(value: &UserValue, interner: &Interner) -> TransformResult {
    let mut new_keys: Option<Vec<(InternedKey, UserKey)>> = None;
    let inner_value = user_to_inner_rec(value, interner, &mut new_keys);
    TransformResult {
        inner_value,
        new_keys,
    }
}

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

