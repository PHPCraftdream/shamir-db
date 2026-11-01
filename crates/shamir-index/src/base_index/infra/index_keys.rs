//! Pure key-encoding helpers for the base index subsystem (identity v2).
//!
//! These are free functions shared by `index_manager` and
//! `index_manager_unique`. They do not access any `IndexManager` fields.
//!
//! # Identity v2 (IDV2, design P34-12 sections (a), (e) S2)
//!
//! The persisted identity is the CANONICAL byte encoding `C` of the indexed
//! value's equality class — the frozen codec of
//! [`crate::model::identity::canonical`], whose numeric classes are the ONE
//! `NumericKey` rule, so `Int(1) == F64(1.0) == Dec(1) == Big(1)` encode
//! identically and the equality decisions are unchanged from the v1/v3 hash
//! scheme. There is NO hash in the inline form: a posting key carries `C`
//! itself (`|C| <= INLINE_MAX`) or `0xFF || BLAKE3-256(C)` (digest form,
//! `|C| > INLINE_MAX`), in which case the posting VALUE carries `C` so every
//! equality decision is made on canonical bytes, never on a hash alone.
//!
//! The write path (`RecordRef` lens: `scalar_at` zero-copy, `materialize_at`
//! for Dec/Big/containers) and the lookup path (`&[InnerValue]` supplied by
//! the engine) call the SAME encoder per leaf class; a parity test pins
//! byte-identity. Top-level NaN has no identity (a NaN matches nothing),
//! exactly as before. The rustc-hash `FxHasher` streams of the v1/v3 scheme
//! are removed from the persisted path completely.
//!
//! # Index format version
//!
//! This is identity VERSION 2 of the base family (marker `_m.idx.idv`,
//! replacing the legacy `_m.idx.lfv`). The engine wires the blocking
//! open-time base migration (`identity_migration.rs`): `need = !(idv == 2
//! && lfv absent)`. It clears both markers in one transaction, then for
//! every regular and unique definition sweeps BOTH the legacy `[0|1][name]`
//! and the v2 `[2|3][name]` namespaces and re-backfills the postings from
//! the data store through the creation path, leaving the persisted
//! definition untouched, and finally sets `idv = 2` (the commit point). A
//! crash before it repeats the rebuild on the next open. The rebuild reads
//! the records through the table's MVCC view (the raw `__data__` store is
//! inert under MVCC).
//!
//! History: v1/v3 keys were `[0|1][name][h1][h2]` with two 64-bit FxHash
//! words (collision-prone, insertion-order dependent for maps); v2 keys are
//! `[2|3][name][I]` with the canonical identity above.

use bytes::Bytes;
use shamir_types::core::interner::InternerKey;
use shamir_types::record_view::RecordRef;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use smallvec::SmallVec;

use crate::base_index::index_info_item::IndexInfoItem;
use crate::base_index::index_record_key::IndexRecordKey;
use crate::model::identity::canonical::{encode_inner_value_into, encode_scalar_ref_into};
use crate::model::identity::identity_bytes::{identity_bytes, INLINE_MAX};

/// Build a regular-family posting key `[2][name][I][rid]`.
pub fn build_posting_key(index_key: &Bytes, record_id: &RecordId) -> Bytes {
    let mut k = Vec::with_capacity(index_key.len() + 16);
    k.extend_from_slice(index_key);
    k.extend_from_slice(record_id.as_bytes());
    Bytes::from(k)
}

/// Canonical bytes `C` of the composite value selected by `paths` from a
/// record (write path). Leaves are concatenated in index path order; each
/// leaf is encoded via `scalar_at` (zero-copy) first, falling back to
/// `materialize_at` for Dec/Big/containers.
///
/// Returns `None` if ANY path is absent (all-or-nothing semantics, as
/// before) or if a leaf has no identity (top-level NaN — a NaN matches
/// nothing). `CodecError::TooLarge` cannot occur under the frame budget and
/// is treated the same way (no posting, never a truncated identity).
pub fn canonical_from_record(
    rec: &(impl RecordRef + ?Sized),
    paths: &[IndexInfoItem],
) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    for item in paths {
        let ipath: SmallVec<[InternerKey; 4]> =
            item.path.iter().map(|&id| InternerKey::new(id)).collect();

        // Fast path: scalar_at is zero-copy.
        if let Some(sr) = rec.scalar_at(&ipath) {
            encode_scalar_ref_into(&sr, &mut buf).ok()?;
            continue;
        }

        // Slow path: materialize the leaf for Dec/Big/containers.
        let leaf = rec.materialize_at(&ipath)?;
        encode_inner_value_into(&leaf, &mut buf).ok()?;
    }
    Some(buf)
}

/// Canonical bytes `C` of lookup values (`&[InnerValue]` supplied by the
/// engine's `lookup_by_index`). Same encoder per class as
/// [`canonical_from_record`]; `None` when a value has no identity (NaN).
pub fn canonical_from_values(values: &[InnerValue]) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    for v in values {
        encode_inner_value_into(v, &mut buf).ok()?;
    }
    Some(buf)
}

/// Whether canonical bytes `C` exceed the inline cap and travel in digest
/// form (so the posting VALUE must carry `C`).
pub fn is_digest_form(canonical: &[u8]) -> bool {
    canonical.len() > INLINE_MAX
}

/// Extract the indexed leaves for a composite index from a record.
///
/// For each `IndexInfoItem` path: convert `&[u64]` to `&[InternerKey]` and
/// call `rec.materialize_at(path)`. If ANY path is missing (returns `None`)
/// the WHOLE record is skipped (returns `None`).
///
/// The engine still uses this for unique-index duplicate detection in a few
/// paths (serializing values for comparison); the hot write/read cycle uses
/// [`build_index_key_from_record`] / [`build_index_key`], which encode
/// directly from the lens without collecting owned leaves.
pub fn extract_index_leaves(
    rec: &(impl RecordRef + ?Sized),
    paths: &[IndexInfoItem],
) -> Option<Vec<InnerValue>> {
    let mut values = Vec::with_capacity(paths.len());
    for item in paths {
        let path: SmallVec<[InternerKey; 4]> =
            item.path.iter().map(|&id| InternerKey::new(id)).collect();
        match rec.materialize_at(&path) {
            Some(v) => values.push(v),
            None => return None,
        }
    }
    Some(values)
}

/// Extract the indexed leaves AND build the v2 identity in one pass.
///
/// For each `IndexInfoItem` path, tries `scalar_at` (zero-copy) then falls
/// back to `materialize_at` (owned, for Dec/Big/containers). If ANY path is
/// absent, or a leaf has no identity (NaN), the WHOLE record is skipped
/// (returns `None`).
///
/// Returns `(IndexRecordKey, C)`: the fully-formed posting key (namespace
/// `0x02`/`0x03` + name + identity bytes `I`) and the canonical bytes `C`
/// the caller needs for the digest-form posting VALUE (regular family:
/// value = `C`; unique family: value = owner `RecordId` + `C`).
pub fn build_index_key_from_record(
    is_unique: bool,
    name_interned: u64,
    rec: &(impl RecordRef + ?Sized),
    paths: &[IndexInfoItem],
) -> Option<(IndexRecordKey, Vec<u8>)> {
    let canonical = canonical_from_record(rec, paths)?;
    let key =
        IndexRecordKey::new(is_unique, name_interned).with_identity(identity_bytes(&canonical));
    Some((key, canonical))
}

/// Build the index key from lookup values (`&[InnerValue]` supplied by
/// the engine's `lookup_by_index`). Uses the SAME encoder as
/// [`build_index_key_from_record`] so write-path and read-path agree.
/// Returns `None` when the probe has no identity (a NaN probe matches
/// nothing — no posting can exist for it, including in digest form, because
/// NaN never produces canonical bytes to digest).
pub fn build_index_key(
    is_unique: bool,
    name_interned: u64,
    values: &[InnerValue],
) -> Option<IndexRecordKey> {
    let canonical = canonical_from_values(values)?;
    Some(IndexRecordKey::new(is_unique, name_interned).with_identity(identity_bytes(&canonical)))
}
