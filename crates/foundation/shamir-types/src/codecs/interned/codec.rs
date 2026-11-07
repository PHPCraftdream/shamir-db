//! Interned codec — QueryValue ↔ InnerValue conversions and RecordView de-intern.
//!
//! This module provides conversions between the engine-native `InnerValue`
//! (interned keys) and the user-facing `QueryValue` (string keys), plus the
//! RecordView lens walker for zero-copy de-intern.
//!
//! The old text-codec functions (`inner_to_value`, `record_view_to_value`,
//! `inner_to_wire`, `wire_value_to_inner`, …) have been removed. All public
//! paths now go through `QueryValue`.

use crate::codecs::interned::common::intern_string_key;
use crate::codecs::CodecError;
use crate::core::interner::{Interner, InternerKey};
use crate::record_view::{RecordValue, RecordView};
use crate::types::common::{new_map_wc, TSet};
use crate::types::value::{InnerValue, QueryValue, Value};
use num_bigint::BigInt;

/// Converts a [`QueryValue`] (string-keyed) to [`InnerValue`] (interned keys).
///
/// This is the key function for the zero-copy write path: once user data
/// is deserialized as `QueryValue` (format-agnostic), this pass interns
/// the map keys to produce the engine-native representation.
pub fn query_value_to_inner(
    qv: &QueryValue,
    interner: &Interner,
) -> Result<InnerValue, CodecError> {
    query_value_to_inner_with(qv, &|key| intern_string_key(interner, key))
}

/// Converts a [`QueryValue`] to [`InnerValue`] using a custom interning function.
pub fn query_value_to_inner_with<F>(
    qv: &QueryValue,
    intern_key: &F,
) -> Result<InnerValue, CodecError>
where
    F: Fn(&str) -> Result<InternerKey, CodecError>,
{
    match qv {
        Value::Null => Ok(InnerValue::Null),
        Value::Bool(b) => Ok(InnerValue::Bool(*b)),
        Value::Int(i) => Ok(InnerValue::Int(*i)),
        Value::F64(f) => Ok(InnerValue::F64(*f)),
        Value::Dec(d) => Ok(InnerValue::Dec(*d)),
        Value::Big(b) => Ok(InnerValue::Big(b.clone())),
        Value::Str(s) => Ok(InnerValue::Str(s.clone())),
        Value::Bin(b) => Ok(InnerValue::Bin(b.clone())),
        Value::List(l) => {
            let converted: Result<Vec<InnerValue>, CodecError> = l
                .iter()
                .map(|v| query_value_to_inner_with(v, intern_key))
                .collect();
            Ok(InnerValue::List(converted?))
        }
        Value::Set(s) => {
            let converted: Result<TSet<InnerValue>, CodecError> = s
                .iter()
                .map(|v| query_value_to_inner_with(v, intern_key))
                .collect();
            Ok(InnerValue::Set(converted?))
        }
        Value::Map(m) => {
            let mut converted = new_map_wc(m.len());
            for (key_str, val) in m {
                let interned_key = intern_key(key_str)?;
                let converted_val = query_value_to_inner_with(val, intern_key)?;
                converted.insert(interned_key, converted_val);
            }
            Ok(InnerValue::Map(converted))
        }
    }
}

/// Converts [`InnerValue`] (interned keys) to [`QueryValue`] (string keys),
/// de-interning map keys via a single reverse-snapshot acquisition.
///
/// Mirrors the semantics of the deleted text-codec path — same
/// key-resolution behaviour and same error handling for missing intern keys —
/// builds the allocation-light `QueryValue` tree as the canonical output.
pub fn inner_value_to_query_value(
    value: &InnerValue,
    interner: &Interner,
) -> Result<QueryValue, CodecError> {
    let rev = interner.reverse_snapshot();
    inner_value_to_query_value_with_resolver(
        value,
        &|id| {
            rev.get(id as usize)
                .and_then(|slot| slot.get())
                .map(ToString::to_string)
        },
        &|id| format!("Interned key not found: {:?}", InternerKey::new(id)),
    )
}

/// Converts an `InnerValue` using a caller-supplied id-to-name resolver.
pub fn inner_value_to_query_value_with<F>(
    value: &InnerValue,
    resolve: &F,
) -> Result<QueryValue, CodecError>
where
    F: Fn(u64) -> Option<String>,
{
    inner_value_to_query_value_with_resolver(value, resolve, &|id| {
        format!("unknown interned id {}", id)
    })
}

fn inner_value_to_query_value_with_resolver<F, E>(
    value: &InnerValue,
    resolve: &F,
    missing: &E,
) -> Result<QueryValue, CodecError>
where
    F: Fn(u64) -> Option<String>,
    E: Fn(u64) -> String,
{
    match value {
        Value::Null => Ok(QueryValue::Null),
        Value::Bool(b) => Ok(QueryValue::Bool(*b)),
        Value::Int(i) => Ok(QueryValue::Int(*i)),
        Value::F64(f) => Ok(QueryValue::F64(*f)),
        Value::Dec(d) => Ok(QueryValue::Dec(*d)),
        Value::Big(b) => Ok(QueryValue::Big(b.clone())),
        Value::Str(s) => Ok(QueryValue::Str(s.clone())),
        Value::Bin(b) => Ok(QueryValue::Bin(b.clone())),
        Value::List(values) => Ok(QueryValue::List(
            values
                .iter()
                .map(|v| inner_value_to_query_value_with_resolver(v, resolve, missing))
                .collect::<Result<_, _>>()?,
        )),
        Value::Set(values) => Ok(QueryValue::Set(
            values
                .iter()
                .map(|v| inner_value_to_query_value_with_resolver(v, resolve, missing))
                .collect::<Result<_, _>>()?,
        )),
        Value::Map(values) => {
            let mut out = new_map_wc(values.len());
            for (key, value) in values {
                let id = key.id();
                let name = resolve(id).ok_or_else(|| CodecError::Decode(missing(id)))?;
                out.insert(
                    name,
                    inner_value_to_query_value_with_resolver(value, resolve, missing)?,
                );
            }
            Ok(QueryValue::Map(out))
        }
    }
}

// ---------------------------------------------------------------------------
// RecordView lens de-intern — O(N) direct walk (no intermediate InnerValue tree)
// ---------------------------------------------------------------------------

/// Converts a [`RecordView`] to [`QueryValue`] using one interner snapshot.
pub fn record_view_to_query_value(
    view: &RecordView<'_>,
    interner: &Interner,
) -> Result<QueryValue, CodecError> {
    let rev = interner.reverse_snapshot();
    record_view_to_query_value_with_resolver(
        view,
        &|id| {
            rev.get(id as usize)
                .and_then(|slot| slot.get())
                .map(ToString::to_string)
        },
        &|id| format!("Interned key not found: {:?}", InternerKey::new(id)),
    )
}

/// Converts a [`RecordView`] using a caller-supplied id-to-name resolver.
pub fn record_view_to_query_value_with<F>(
    view: &RecordView<'_>,
    resolve: &F,
) -> Result<QueryValue, CodecError>
where
    F: Fn(u64) -> Option<String>,
{
    record_view_to_query_value_with_resolver(view, resolve, &|id| {
        format!("unknown interned id {} (client cache miss)", id)
    })
}

fn record_view_to_query_value_with_resolver<F, E>(
    view: &RecordView<'_>,
    resolve: &F,
    missing: &E,
) -> Result<QueryValue, CodecError>
where
    F: Fn(u64) -> Option<String>,
    E: Fn(u64) -> String,
{
    let mut obj = new_map_wc(view.len().min(view.body_len()));
    for (key, value) in view.fields() {
        let id = key.id();
        let name = resolve(id).ok_or_else(|| CodecError::Decode(missing(id)))?;
        obj.insert(name, rv_deintern_value_with(&value, resolve, missing)?);
    }
    Ok(QueryValue::Map(obj))
}

fn rv_deintern_value_with<F, E>(
    rv: &RecordValue<'_>,
    resolve: &F,
    missing: &E,
) -> Result<QueryValue, CodecError>
where
    F: Fn(u64) -> Option<String>,
    E: Fn(u64) -> String,
{
    match rv {
        RecordValue::Null => Ok(QueryValue::Null),
        RecordValue::Bool(b) => Ok(QueryValue::Bool(*b)),
        RecordValue::Int(i) => Ok(QueryValue::Int(*i)),
        RecordValue::Big(u) => Ok(QueryValue::Big(BigInt::from(*u))),
        RecordValue::F64(f) => Ok(QueryValue::F64(*f)),
        RecordValue::Str(cow) => Ok(QueryValue::Str(cow.as_ref().to_owned())),
        RecordValue::Bin(b) => Ok(QueryValue::Bin(b.to_vec())),
        RecordValue::Arr(seq) => {
            let mut items = Vec::with_capacity(seq.len().min(seq.as_bytes().len()));
            for elem in seq.iter() {
                items.push(rv_deintern_value_with(&elem, resolve, missing)?);
            }
            Ok(QueryValue::List(items))
        }
        RecordValue::Map(nested) => {
            record_view_to_query_value_with_resolver(nested, resolve, missing)
        }
    }
}

// ---------------------------------------------------------------------------
// FieldMap-backed de-intern — closure-driven variant for the client
// ---------------------------------------------------------------------------

/// De-intern a [`RecordView`] with client-cache error semantics.
pub fn record_view_deintern_with<F>(
    view: &RecordView<'_>,
    resolve: &F,
) -> Result<QueryValue, CodecError>
where
    F: Fn(u64) -> Option<String>,
{
    record_view_to_query_value_with(view, resolve)
}
