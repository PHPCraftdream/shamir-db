//! Pure key-encoding helpers for the index subsystem.
//!
//! These are free functions shared by `index_manager` and
//! `index_manager_unique`. They do not access any `IndexManager` fields.
//!
//! # S9 — lens-native leaf hashing
//!
//! Index keys are hashed via a STABLE, deterministic, version-local scheme
//! that works directly from the `RecordRef` lens:
//!
//! - **Scalars** (Null, Bool, Int, F64, Str, Bin) are hashed via
//!   `scalar_at` (zero-copy `ScalarRef`) — no owned `InnerValue` needed.
//! - **Non-scalar leaves** (Dec, Big, containers) are hashed via
//!   `materialize_at` → transient owned value → `hash_inner_value`.
//!   This is the ONE unavoidable `InnerValue` usage — the `RecordRef` trait's
//!   escape hatch method returns `InnerValue` by design. The value is consumed
//!   immediately; no InnerValue is stored or persisted.
//!
//! The hash uses EXPLICIT u8 discriminant tags (0..=10), NOT
//! `std::mem::discriminant`, so it is stable across rustc versions and enum
//! layout changes. Set and Map hashes are order-independent (XOR of per-element
//! hashes).
//!
//! The lookup path (`lookup_by_index`, `check_unique_constraint`) receives
//! `&[InnerValue]` from the engine and feeds each through `hash_inner_value`
//! with the SAME tag scheme, so write-path and read-path hashes agree.
//!
//! # Index format version
//!
//! This hash scheme is VERSION 2 of the legacy index posting format.
//! Old V1 postings (based on `<Value<InternerKey> as Hash>` with
//! `std::mem::discriminant` tags) are NOT compatible — the engine triggers
//! a full O(N) rebuild-on-open when it detects a version mismatch (see
//! `LEGACY_INDEX_FORMAT_VERSION` in `persistence.rs`).

use crate::legacy::index_info_item::IndexInfoItem;
use crate::legacy::index_record_key::IndexRecordKey;
use bytes::Bytes;
use fxhash::FxHasher;
use shamir_types::core::interner::InternerKey;
use shamir_types::record_view::RecordRef;
use shamir_types::record_view::ScalarRef;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use smallvec::SmallVec;
use std::hash::{Hash, Hasher};

// ============================================================================
// Stable discriminant tags — explicit u8, never `std::mem::discriminant`.
// ============================================================================

const TAG_NULL: u8 = 0;
const TAG_BOOL: u8 = 1;
const TAG_INT: u8 = 2;
const TAG_F64: u8 = 3;
const TAG_STR: u8 = 4;
const TAG_BIN: u8 = 5;
const TAG_DEC: u8 = 6;
const TAG_BIG: u8 = 7;
const TAG_LIST: u8 = 8;
const TAG_SET: u8 = 9;
const TAG_MAP: u8 = 10;

// ============================================================================
// Core hash primitives
// ============================================================================

/// Hash a `ScalarRef` leaf into the hasher using stable tags.
/// Covers: Null, Bool, Int, F64, Str, Bin.
fn hash_scalar_ref(sr: &ScalarRef<'_>, state: &mut impl Hasher) {
    match sr {
        ScalarRef::Null => {
            TAG_NULL.hash(state);
        }
        ScalarRef::Bool(b) => {
            TAG_BOOL.hash(state);
            b.hash(state);
        }
        ScalarRef::Int(i) => {
            TAG_INT.hash(state);
            i.hash(state);
        }
        ScalarRef::F64(f) => {
            TAG_F64.hash(state);
            f.to_bits().hash(state);
        }
        ScalarRef::Str(s) => {
            TAG_STR.hash(state);
            s.hash(state);
        }
        ScalarRef::Bin(b) => {
            TAG_BIN.hash(state);
            b.hash(state);
        }
    }
}

/// Hash an `InnerValue` leaf into the hasher using the SAME stable tags
/// as `hash_scalar_ref`, so the write path (ScalarRef) and the lookup
/// path (InnerValue) produce IDENTICAL hashes for equivalent values.
///
/// Set and Map hashes are order-independent: each element is hashed
/// independently with a fresh FxHasher, the results are XOR'd, and the
/// XOR sum is fed into `state`.
fn hash_inner_value(v: &InnerValue, state: &mut impl Hasher) {
    match v {
        InnerValue::Null => {
            TAG_NULL.hash(state);
        }
        InnerValue::Bool(b) => {
            TAG_BOOL.hash(state);
            b.hash(state);
        }
        InnerValue::Int(i) => {
            TAG_INT.hash(state);
            i.hash(state);
        }
        InnerValue::F64(f) => {
            TAG_F64.hash(state);
            f.to_bits().hash(state);
        }
        InnerValue::Str(s) => {
            TAG_STR.hash(state);
            s.hash(state);
        }
        InnerValue::Bin(b) => {
            TAG_BIN.hash(state);
            b.hash(state);
        }
        InnerValue::Dec(d) => {
            TAG_DEC.hash(state);
            d.hash(state);
        }
        InnerValue::Big(b) => {
            TAG_BIG.hash(state);
            b.hash(state);
        }
        InnerValue::List(l) => {
            TAG_LIST.hash(state);
            for elem in l {
                hash_inner_value(elem, state);
            }
        }
        InnerValue::Set(s) => {
            TAG_SET.hash(state);
            let mut xor_sum: u64 = 0;
            for elem in s {
                let mut h = FxHasher::default();
                hash_inner_value(elem, &mut h);
                xor_sum ^= h.finish();
            }
            xor_sum.hash(state);
        }
        InnerValue::Map(m) => {
            TAG_MAP.hash(state);
            let mut xor_sum: u64 = 0;
            for (k, v) in m {
                let mut h = FxHasher::default();
                // Hash the InternerKey's numeric id directly — deterministic
                // and order-independent.
                k.id().hash(&mut h);
                hash_inner_value(v, &mut h);
                xor_sum ^= h.finish();
            }
            xor_sum.hash(state);
        }
    }
}

// ============================================================================
// Dual-hash computation (hash1 + hash2 with different seeds)
// ============================================================================

/// Compute the dual `(hash1, hash2)` for one or more leaves hashed from
/// a `RecordRef`. Each leaf is hashed via `scalar_at` when possible (the
/// fast zero-copy path); falls back to `materialize_at` for Dec/Big/containers.
///
/// Returns `None` if ANY path is absent (all-or-nothing semantics).
fn compute_leaf_hashes(
    rec: &(impl RecordRef + ?Sized),
    paths: &[IndexInfoItem],
    name_interned: u64,
) -> Option<(u64, u64)> {
    const SEED2: u64 = 0x9E3779B97F4A7C15;

    let mut h1 = FxHasher::default();
    let mut h2 = FxHasher::default();

    SEED2.hash(&mut h2);
    name_interned.hash(&mut h2);
    name_interned.hash(&mut h1);

    for item in paths {
        let ipath: SmallVec<[InternerKey; 4]> =
            item.path.iter().map(|&id| InternerKey::new(id)).collect();

        // Fast path: scalar_at is zero-copy.
        if let Some(sr) = rec.scalar_at(&ipath) {
            hash_scalar_ref(&sr, &mut h1);
            hash_scalar_ref(&sr, &mut h2);
            continue;
        }

        // Slow path: materialize the leaf for Dec/Big/containers.
        // This is the ONE unavoidable InnerValue usage (RecordRef trait
        // returns InnerValue from materialize_at by design).
        let leaf = rec.materialize_at(&ipath)?;
        hash_inner_value(&leaf, &mut h1);
        hash_inner_value(&leaf, &mut h2);
    }

    Some((h1.finish(), h2.finish()))
}

/// Compute the dual `(hash1, hash2)` for lookup values (`&[InnerValue]`)
/// using the SAME tag scheme as `compute_leaf_hashes`.
fn compute_lookup_hashes(values: &[InnerValue], name_interned: u64) -> (u64, u64) {
    const SEED2: u64 = 0x9E3779B97F4A7C15;

    let mut h1 = FxHasher::default();
    let mut h2 = FxHasher::default();

    SEED2.hash(&mut h2);
    name_interned.hash(&mut h2);
    name_interned.hash(&mut h1);

    for v in values {
        hash_inner_value(v, &mut h1);
        hash_inner_value(v, &mut h2);
    }

    (h1.finish(), h2.finish())
}

// ============================================================================
// Public API
// ============================================================================

/// Extract the indexed leaves for a composite index from a record AND
/// compute the dual posting-key hash in one pass — no intermediate
/// `Vec<InnerValue>`.
///
/// For each `IndexInfoItem` path, tries `scalar_at` (zero-copy) then
/// falls back to `materialize_at` (owned, for Dec/Big/containers). If
/// ANY path is absent the WHOLE record is skipped (all-or-nothing).
///
/// Returns the fully-formed `IndexRecordKey` or `None`.
pub fn build_index_key_from_record(
    is_unique: bool,
    name_interned: u64,
    rec: &(impl RecordRef + ?Sized),
    paths: &[IndexInfoItem],
) -> Option<IndexRecordKey> {
    let (h1, h2) = compute_leaf_hashes(rec, paths, name_interned)?;
    Some(IndexRecordKey::new(is_unique, name_interned).with_hash(h1, h2))
}

/// Build the index key from lookup values (`&[InnerValue]` supplied by
/// the engine's `lookup_by_index`). Uses the SAME hash scheme as
/// `build_index_key_from_record` so write-path and read-path agree.
pub(super) fn build_index_key(
    is_unique: bool,
    name_interned: u64,
    values: &[InnerValue],
) -> IndexRecordKey {
    let (h1, h2) = compute_lookup_hashes(values, name_interned);
    IndexRecordKey::new(is_unique, name_interned).with_hash(h1, h2)
}

/// Extract the indexed leaves for a composite index from a record.
///
/// For each `IndexInfoItem` path: convert `&[u64]` to `&[InternerKey]` and
/// call `rec.materialize_at(path)`. If ANY path is missing (returns `None`)
/// the WHOLE record is skipped (returns `None`).
///
/// **S9 note**: this function still returns `Vec<InnerValue>` because the
/// engine uses it for unique-index duplicate detection (serializing values
/// for comparison). The hot-path write/read cycle uses
/// `build_index_key_from_record` instead, which hashes directly from the
/// lens without collecting owned leaves.
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

/// Compose the physical posting key:
/// `index_key (25b) || record_id (16b)` = 41 bytes.
pub(super) fn build_posting_key(index_key: &Bytes, record_id: &RecordId) -> Bytes {
    let mut k = Vec::with_capacity(index_key.len() + 16);
    k.extend_from_slice(index_key);
    k.extend_from_slice(record_id.as_bytes());
    Bytes::from(k)
}
