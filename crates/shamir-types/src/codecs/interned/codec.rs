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
use crate::record_view::{RawSeq, RecordValue, RecordView};
use crate::types::common::{new_map_wc, TSet};
use crate::types::value::{InnerValue, QueryValue, Value};
use std::sync::{Arc, OnceLock};

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
    inner_value_to_query_value_with_rev(value, rev.as_slice())
}

fn inner_value_to_query_value_with_rev(
    value: &InnerValue,
    rev: &[OnceLock<Arc<str>>],
) -> Result<QueryValue, CodecError> {
    match value {
        Value::Null => Ok(QueryValue::Null),
        Value::Bool(b) => Ok(QueryValue::Bool(*b)),
        Value::Int(i) => Ok(QueryValue::Int(*i)),
        Value::F64(f) => Ok(QueryValue::F64(*f)),
        Value::Dec(d) => Ok(QueryValue::Dec(*d)),
        Value::Big(b) => Ok(QueryValue::Big(b.clone())),
        Value::Str(s) => Ok(QueryValue::Str(s.clone())),
        Value::Bin(b) => Ok(QueryValue::Bin(b.clone())),
        Value::List(l) => {
            let arr: Result<Vec<_>, _> = l
                .iter()
                .map(|v| inner_value_to_query_value_with_rev(v, rev))
                .collect();
            Ok(QueryValue::List(arr?))
        }
        Value::Set(s) => {
            let converted: Result<TSet<QueryValue>, _> = s
                .iter()
                .map(|v| inner_value_to_query_value_with_rev(v, rev))
                .collect();
            Ok(QueryValue::Set(converted?))
        }
        Value::Map(m) => {
            let mut obj = new_map_wc(m.len());
            for (interned_key, val) in m {
                let idx = interned_key.id() as usize;
                let key_str = rev
                    .get(idx)
                    .and_then(|slot| slot.get())
                    .map(|k| k.to_string())
                    .ok_or_else(|| {
                        CodecError::Decode(format!("Interned key not found: {:?}", interned_key))
                    })?;
                obj.insert(key_str, inner_value_to_query_value_with_rev(val, rev)?);
            }
            Ok(QueryValue::Map(obj))
        }
    }
}

// ---------------------------------------------------------------------------
// RecordView lens de-intern — O(N) direct walk (no intermediate InnerValue tree)
// ---------------------------------------------------------------------------

/// Converts a [`RecordView`] (id-keyed msgpack lens) to [`QueryValue`] (string keys)
/// in a single O(N) pass over [`RecordView::fields`], resolving key ids via a
/// single `reverse_snapshot` acquisition. Mirrors `inner_value_to_query_value`
/// arm-for-arm so that lens-path == tree-path on every shape the storage encoder
/// can produce.
pub fn record_view_to_query_value(
    view: &RecordView<'_>,
    interner: &Interner,
) -> Result<QueryValue, CodecError> {
    let rev = interner.reverse_snapshot();
    record_view_to_query_value_with_rev(view, rev.as_slice())
}

fn record_view_to_query_value_with_rev(
    view: &RecordView<'_>,
    rev: &[OnceLock<Arc<str>>],
) -> Result<QueryValue, CodecError> {
    let mut obj = new_map_wc(view.len());
    for (interned_key, rv) in view.fields() {
        let idx = interned_key.id() as usize;
        let key_str = rev
            .get(idx)
            .and_then(|slot| slot.get())
            .map(|k| k.to_string())
            .ok_or_else(|| {
                CodecError::Decode(format!("Interned key not found: {:?}", interned_key))
            })?;
        obj.insert(key_str, record_value_to_query_value_with_rev(&rv, rev)?);
    }
    Ok(QueryValue::Map(obj))
}

/// Convert a single [`RecordValue`] to [`QueryValue`], recursing into nested
/// maps (via `record_view_to_query_value_with_rev`) and arrays.
fn record_value_to_query_value_with_rev(
    rv: &RecordValue<'_>,
    rev: &[OnceLock<Arc<str>>],
) -> Result<QueryValue, CodecError> {
    match rv {
        RecordValue::Null => Ok(QueryValue::Null),
        RecordValue::Bool(b) => Ok(QueryValue::Bool(*b)),
        RecordValue::Int(i) => Ok(QueryValue::Int(*i)),
        RecordValue::F64(f) => Ok(QueryValue::F64(*f)),
        RecordValue::Str(cow) => Ok(QueryValue::Str(cow.as_ref().to_owned())),
        RecordValue::Bin(b) => Ok(QueryValue::Bin(b.to_vec())),
        RecordValue::Arr(seq) => convert_raw_seq_to_query_value(seq, rev),
        RecordValue::Map(nested) => record_view_to_query_value_with_rev(nested, rev),
    }
}

/// Walk a [`RawSeq`] and convert each element to [`QueryValue`].
fn convert_raw_seq_to_query_value(
    seq: &RawSeq<'_>,
    rev: &[OnceLock<Arc<str>>],
) -> Result<QueryValue, CodecError> {
    let mut items = Vec::with_capacity(seq.len());
    for elem in seq.iter() {
        items.push(record_value_to_query_value_with_rev(&elem, rev)?);
    }
    Ok(QueryValue::List(items))
}

// ---------------------------------------------------------------------------
// FieldMap-backed de-intern — closure-driven variant for the client
// ---------------------------------------------------------------------------

/// Converts a [`RecordView`] (id-keyed msgpack lens) to [`QueryValue`] (string
/// keys) using a caller-supplied resolver instead of an [`Interner`].
///
/// Added for the client-side de-intern path (S-client): the client holds a
/// [`FieldMap`] (not an `Interner`), so instead of taking a reverse-snapshot
/// vec the caller passes a `Fn(u64) -> Option<&str>` that resolves each
/// id to a name from the FieldMap. The function returns
/// `Err(CodecError::Decode("unknown id …"))` for any id the resolver cannot
/// resolve — the caller can then refresh the interner cache and retry.
///
/// Justification for the new public function (rather than reusing
/// `record_view_to_query_value`): the `Interner` type is server-only
/// (`shamir-engine` / `shamir-server`); the client has a `FieldMap`
/// (`scc::HashMap<u64, String>`). Threading the `Interner` into the client
/// crate would create an unwanted cross-crate dependency. A closure-based
/// variant is the lightest approach that avoids that coupling while reusing
/// the proven O(N) lens walk.
pub fn record_view_deintern_with<F>(
    view: &RecordView<'_>,
    resolve: &F,
) -> Result<QueryValue, CodecError>
where
    F: Fn(u64) -> Option<String>,
{
    let mut obj = new_map_wc(view.len());
    for (interned_key, rv) in view.fields() {
        let id = interned_key.id();
        let key_str = resolve(id).ok_or_else(|| {
            CodecError::Decode(format!("unknown interned id {} (client cache miss)", id))
        })?;
        obj.insert(key_str, rv_deintern_value_with(&rv, resolve)?);
    }
    Ok(QueryValue::Map(obj))
}

/// Recursively de-intern a [`RecordValue`] using the resolver closure.
fn rv_deintern_value_with<F>(rv: &RecordValue<'_>, resolve: &F) -> Result<QueryValue, CodecError>
where
    F: Fn(u64) -> Option<String>,
{
    match rv {
        RecordValue::Null => Ok(QueryValue::Null),
        RecordValue::Bool(b) => Ok(QueryValue::Bool(*b)),
        RecordValue::Int(i) => Ok(QueryValue::Int(*i)),
        RecordValue::F64(f) => Ok(QueryValue::F64(*f)),
        RecordValue::Str(cow) => Ok(QueryValue::Str(cow.as_ref().to_owned())),
        RecordValue::Bin(b) => Ok(QueryValue::Bin(b.to_vec())),
        RecordValue::Arr(seq) => rv_deintern_seq_with(seq, resolve),
        RecordValue::Map(nested) => record_view_deintern_with(nested, resolve),
    }
}

/// Walk a [`RawSeq`] using the resolver closure.
fn rv_deintern_seq_with<F>(seq: &RawSeq<'_>, resolve: &F) -> Result<QueryValue, CodecError>
where
    F: Fn(u64) -> Option<String>,
{
    let mut items = Vec::with_capacity(seq.len());
    for elem in seq.iter() {
        items.push(rv_deintern_value_with(&elem, resolve)?);
    }
    Ok(QueryValue::List(items))
}
