use bytes::Bytes;
use smallvec::SmallVec;

use super::SortedIndexDefinition;
use crate::base_index::sorted_index_definition::SORTED_TAG;
use shamir_storage::error::DbResult;
use shamir_types::core::interner::InternerKey;
use shamir_types::core::sort_codec;
use shamir_types::record_view::{RecordRef, ScalarRef};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

/// Serialised covering-index projection type: a list of
/// `(field_path_dotted, QueryValue)` pairs, encoded with MessagePack.
///
/// S9: changed from `Vec<(String, InnerValue)>` to `Vec<(String, QueryValue)>`
/// as part of the InnerValue-elimination campaign. `QueryValue = Value<String>`
/// has NO `InternerKey` dependency.
///
/// The field_path_dotted key is the segments joined with "." so the
/// read-side (S3.3) can reconstruct the projection without the interner.
///
/// Format: `rmp_serde::to_vec_named(&Vec<(String, QueryValue)>)` — bincode is
/// not usable because `Value`'s `Deserialize` relies on `deserialize_any`,
/// which bincode does not support.
///
/// The wire format for SCALAR leaves (Null, Bool, Int, F64, Str, Bin, Dec, Big)
/// is byte-identical between `QueryValue` and `InnerValue`, so the decode side
/// (`decode_covering_projection`) can deserialize as `Vec<(String, InnerValue)>`
/// without conversion. Container leaves are skipped at encode time (they would
/// differ due to key types).
pub(super) type CoveringProjection = Vec<(String, QueryValue)>;

/// Build the covering-index projection value for one record and one
/// `SortedIndexDefinition` that has non-empty `included_fields_interned`.
///
/// S9: produces `Vec<(String, QueryValue)>` instead of `Vec<(String, InnerValue)>`.
/// For each included field path:
///   - Walk `record` by the interned path segments via `RecordRef::materialize_at`.
///   - Convert the leaf to `QueryValue` (scalar-only; containers are skipped).
///   - If the leaf is present, push `(path_joined_with_dots, leaf)` (owned).
///   - Missing / container paths are silently skipped.
///
/// Returns `Bytes::new()` when no fields could be resolved (backward-compat:
/// write side acts as if no projection; read side sees empty value).
///
/// When the projection is non-empty the returned bytes are a **versioned
/// envelope**:
/// ```text
/// [8 bytes: version as u64 little-endian] ++ [msgpack: Vec<(String, QueryValue)>]
/// ```
/// The `version` parameter should be the MVCC write version for the record
/// being indexed (pass `0` when no MVCC store is attached).
pub(super) fn build_covering_projection(
    record: &(impl RecordRef + ?Sized),
    def: &SortedIndexDefinition,
    version: u64,
) -> Bytes {
    let mut projection: CoveringProjection = Vec::new();
    for (path_strs, path_ids) in def
        .included_fields
        .iter()
        .zip(def.included_fields_interned.iter())
    {
        if path_ids.is_empty() {
            continue;
        }
        let ipath: SmallVec<[InternerKey; 4]> =
            path_ids.iter().map(|&id| InternerKey::new(id)).collect();
        if let Some(leaf) = record.materialize_at(&ipath) {
            if let Some(qv) = inner_value_to_query_scalar(&leaf) {
                let key_str = path_strs.join(".");
                projection.push((key_str, qv));
            }
            // Container leaves (Map/List/Set) are skipped — their QueryValue
            // wire format differs from InnerValue due to key types, and the
            // decode side reads as InnerValue. Scalar leaves are wire-identical.
        }
    }
    if projection.is_empty() {
        return Bytes::new();
    }
    // Use MessagePack (rmp_serde) because Value's Deserialize impl
    // relies on `deserialize_any`, which bincode does not support.
    match rmp_serde::to_vec_named(&projection) {
        Ok(msgpack) => {
            let mut out = version.to_le_bytes().to_vec();
            out.extend_from_slice(&msgpack);
            Bytes::from(out)
        }
        Err(_) => Bytes::new(),
    }
}

/// Convert an `InnerValue` to `QueryValue` for SCALAR types only.
/// Returns `None` for Map/List/Set containers (whose wire format would
/// differ due to InternerKey vs String keys).
fn inner_value_to_query_scalar(v: &InnerValue) -> Option<QueryValue> {
    match v {
        InnerValue::Null => Some(QueryValue::Null),
        InnerValue::Bool(b) => Some(QueryValue::Bool(*b)),
        InnerValue::Int(i) => Some(QueryValue::Int(*i)),
        InnerValue::F64(f) => Some(QueryValue::F64(*f)),
        InnerValue::Dec(d) => Some(QueryValue::Dec(*d)),
        InnerValue::Big(b) => Some(QueryValue::Big(b.clone())),
        InnerValue::Str(s) => Some(QueryValue::Str(s.clone())),
        InnerValue::Bin(b) => Some(QueryValue::Bin(b.clone())),
        // Container leaves are skipped — see doc on build_covering_projection.
        InnerValue::List(_) | InnerValue::Set(_) | InnerValue::Map(_) => None,
    }
}

/// Decode a versioned covering-projection envelope written by
/// `build_covering_projection`. Returns `None` for an empty value, a
/// value shorter than 8 bytes, or one whose msgpack body fails to
/// decode (callers treat `None` as "fall back to a full fetch").
///
/// S9: the encode side writes `Vec<(String, QueryValue)>` but the wire
/// format for scalar leaves is byte-identical to `Vec<(String, InnerValue)>`,
/// so this function can deserialize as `InnerValue` without conversion.
/// The return type stays `InnerValue` for engine API compatibility.
///
/// Used by slice A3 (index-only read path).
pub fn decode_covering_projection(value: &[u8]) -> Option<(u64, Vec<(String, InnerValue)>)> {
    if value.len() < 8 {
        return None;
    }
    let version = u64::from_le_bytes(value[..8].try_into().ok()?);
    let projection: Vec<(String, InnerValue)> = rmp_serde::from_slice(&value[8..]).ok()?;
    Some((version, projection))
}

/// Extract the value at `field_path` from a record and encode it via
/// `sort_codec`. Returns `None` if the field is missing or has a type
/// we don't index (we intentionally skip such records — they won't
/// surface in sorted lookups).
///
/// Reads the scalar via `RecordRef::scalar_at` and dispatches to the
/// SAME `sort_codec::encode_*` primitives the base_index `&InnerValue` path
/// used. `scalar_at` yields exactly {Null, Bool, Int, F64, Str, Bin} for
/// comparable scalars and `None` for Dec/Big/containers/absent —
/// byte-identical to the previous `resolve_path_ref` + InnerValue match,
/// including all the skip cases.
pub(super) fn extract_and_encode(
    rec: &(impl RecordRef + ?Sized),
    field_path: &[u64],
) -> DbResult<Option<Vec<u8>>> {
    let ipath: SmallVec<[InternerKey; 4]> =
        field_path.iter().map(|&id| InternerKey::new(id)).collect();
    let Some(sr) = rec.scalar_at(&ipath) else {
        return Ok(None);
    };
    let mut buf = Vec::with_capacity(16);
    match sr {
        ScalarRef::Null => sort_codec::encode_null(&mut buf),
        ScalarRef::Bool(b) => sort_codec::encode_bool(&mut buf, b),
        ScalarRef::Int(i) => sort_codec::encode_i64(&mut buf, i),
        ScalarRef::F64(f) => {
            if sort_codec::encode_f64(&mut buf, f).is_err() {
                return Ok(None);
            }
        }
        ScalarRef::Str(s) => sort_codec::encode_str(&mut buf, s),
        ScalarRef::Bin(b) => sort_codec::encode_bytes(&mut buf, b),
    }
    Ok(Some(buf))
}

/// The encoded-value slice of a sorted-index physical key
/// `[tag|name|encoded_value|record_id]`: everything between the
/// `value_start` offset (== prefix length) and the trailing 16-byte
/// record_id. Returns `&[]` for a malformed / too-short key.
pub(super) fn key_value_slice(key_bytes: &[u8], value_start: usize) -> &[u8] {
    if key_bytes.len() < value_start + 16 {
        return &[];
    }
    &key_bytes[value_start..key_bytes.len() - 16]
}

/// Decide whether a boundary-value physical entry (one whose encoded value
/// equals the seek value) must be SKIPPED, given the optional record-id
/// tie-breaker `after_id` (task #537).
///
/// * `after_id == None` → skip every boundary row (today's backward-compatible
///   skip-all-ties behavior — an old client that sent no tie-breaker).
/// * `after_id == Some(a)`, `forward == true` (ASC) → skip only rows whose
///   record_id is `<= a` (already seen / the boundary row itself). Rows tied
///   on the seek value with `record_id > a` are RETURNED.
/// * `after_id == Some(a)`, `forward == false` (DESC) → skip only rows whose
///   record_id is `>= a`. Rows tied on the seek value with `record_id < a`
///   are RETURNED.
///
/// A key whose record-id suffix can't be decoded is skipped (malformed).
pub(super) fn skip_boundary_row(
    key_bytes: &[u8],
    after_id: Option<&RecordId>,
    forward: bool,
) -> bool {
    match after_id {
        None => true,
        Some(a) => match decode_record_id_suffix(key_bytes) {
            Some(rid) => {
                if forward {
                    // ASC: strictly past means record_id > a; skip otherwise.
                    rid <= *a
                } else {
                    // DESC: strictly past means record_id < a; skip otherwise.
                    rid >= *a
                }
            }
            None => true,
        },
    }
}

pub(super) fn decode_record_id_suffix(key_bytes: &[u8]) -> Option<RecordId> {
    if key_bytes.len() < 16 {
        return None;
    }
    let tail = &key_bytes[key_bytes.len() - 16..];
    let mut arr = [0u8; 16];
    arr.copy_from_slice(tail);
    Some(RecordId(arr))
}

/// F-67 (#893): decode the `name_interned` (u64, big-endian) out of a
/// physical sorted-index key `[SORTED_TAG (1) | name_interned (8 BE) |
/// encoded_value | record_id (16)]` — the exact layout [`SortedIndexManager
/// ::build_entry_key`]/[`SortedIndexManager::entry_prefix`] write.
///
/// Used by [`SortedIndexManager::bump_touched_indexes`] to recover per-index
/// identity from the `IndexWriteOp::{SetPosting, RemovePosting}` keys the
/// manager's own planners produced — `IndexWriteOp` itself carries no
/// separate index-id field (it's a pure-data enum shared across every index
/// family: hash, sorted, FTS, functional), so the physical key is the only
/// place the identity survives once ops are collected into a flat
/// `Vec<IndexWriteOp>` (see `commit_phases.rs::apply_index_batch`, which
/// resolves this the same way for the tx-commit path).
///
/// Returns `None` for a key that is too short or is not `SORTED_TAG`-prefixed
/// (a foreign key from a different index family sharing the same
/// `info_store` — should never reach this manager's own planner-produced
/// ops, but decoding defensively rather than panicking).
pub(super) fn decode_sorted_index_name(key_bytes: &[u8]) -> Option<u64> {
    if key_bytes.len() < 1 + 8 || key_bytes[0] != SORTED_TAG {
        return None;
    }
    let mut arr = [0u8; 8];
    arr.copy_from_slice(&key_bytes[1..9]);
    Some(u64::from_be_bytes(arr))
}
