//! Validate that every interned map key in a [`RecordView`] resolves in the
//! server's interner reverse snapshot.
//!
//! This is the **S-write security spine**: before a verbatim id-msgpack payload
//! is written to storage, the server must confirm that EVERY key id the client
//! claims is actually known to the interner. A forged or stale id would silently
//! corrupt the on-disk record. The lens walk is O(fields) and allocates nothing.

use crate::codecs::CodecError;
use crate::core::interner::{Interner, InternerKey};
use crate::record_view::{RawSeq, RecordValue, RecordView};
use std::sync::{Arc, OnceLock};

// ---------------------------------------------------------------------------
// Public surface
// ---------------------------------------------------------------------------

/// Validate that every map-key id in `view` (recursively) resolves to a known
/// entry in the interner's reverse snapshot `rev`.
///
/// `rev` is obtained from [`Interner::reverse_snapshot`] — index = id,
/// entry is a set `OnceLock<Arc<str>>` when the id is interned.
///
/// A key id `k` resolves iff `rev.get(k as usize).and_then(|s| s.get()).is_some()`.
///
/// The walk covers:
/// - top-level `(InternerKey, RecordValue)` pairs from `view.fields()` — the
///   KEY id is checked, then the value is inspected;
/// - `RecordValue::Map(nested)` — recurse into the nested view (its keys are
///   also interned ids);
/// - `RecordValue::Arr(seq)` — iterate `seq.iter()` and recurse into any
///   `RecordValue::Map` elements (list elements may themselves be maps with
///   interned keys). Scalar/Str/Bin elements have no keys and are skipped.
///
/// Returns `Err(CodecError::Decode("unresolved interner key id N"))` on the
/// FIRST unresolved key. Returns `Ok(())` when all keys resolve.
pub fn validate_keys_resolve(
    view: &RecordView<'_>,
    rev: &[OnceLock<Arc<str>>],
) -> Result<(), CodecError> {
    for (key, rv) in view.fields() {
        check_key_resolves(&key, rev)?;
        check_record_value_keys(&rv, rev)?;
    }
    Ok(())
}

/// Convenience wrapper that acquires a single `reverse_snapshot()` from
/// `interner` and delegates to [`validate_keys_resolve`].
///
/// Mirrors how [`record_view_to_query_value`](crate::codecs::interned::record_view_to_query_value)
/// wraps its `_with_rev` twin — one `ArcSwap` load for the whole walk.
pub fn validate_keys_resolve_interner(
    view: &RecordView<'_>,
    interner: &Interner,
) -> Result<(), CodecError> {
    let rev = interner.reverse_snapshot();
    validate_keys_resolve(view, rev.as_slice())
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Assert that `key.id()` has a live entry in `rev`.
#[inline]
fn check_key_resolves(key: &InternerKey, rev: &[OnceLock<Arc<str>>]) -> Result<(), CodecError> {
    let id = key.id();
    let resolves = rev.get(id as usize).and_then(|slot| slot.get()).is_some();
    if !resolves {
        return Err(CodecError::Decode(format!(
            "unresolved interner key id {id}"
        )));
    }
    Ok(())
}

/// Recurse into a `RecordValue` and validate any interned map keys it contains.
/// Scalars, Str, and Bin have no interned keys — nothing to check.
fn check_record_value_keys(
    rv: &RecordValue<'_>,
    rev: &[OnceLock<Arc<str>>],
) -> Result<(), CodecError> {
    match rv {
        RecordValue::Map(nested) => validate_keys_resolve(nested, rev),
        RecordValue::Arr(seq) => check_seq_keys(seq, rev),
        // Scalar variants: no keys.
        RecordValue::Null
        | RecordValue::Bool(_)
        | RecordValue::Int(_)
        | RecordValue::F64(_)
        | RecordValue::Str(_)
        | RecordValue::Bin(_) => Ok(()),
    }
}

/// Walk a [`RawSeq`] and recurse into any `Map` elements it contains.
fn check_seq_keys(seq: &RawSeq<'_>, rev: &[OnceLock<Arc<str>>]) -> Result<(), CodecError> {
    for elem in seq.iter() {
        match elem {
            RecordValue::Map(nested) => validate_keys_resolve(&nested, rev)?,
            RecordValue::Arr(inner_seq) => check_seq_keys(&inner_seq, rev)?,
            // Scalars in a list: no keys.
            _ => {}
        }
    }
    Ok(())
}
