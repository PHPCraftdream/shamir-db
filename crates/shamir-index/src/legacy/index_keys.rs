//! Pure key-encoding helpers for the index subsystem.
//!
//! These are free functions shared by `index_manager` and
//! `index_manager_unique`. They do not access any `IndexManager` fields.
//!
//! # Byte-identity (W2a hash/unique)
//!
//! Legacy + unique index keys are PERSISTED. The legacy key is
//! `FxHash(<InnerValue as Hash>::hash(leaf))` with a leading
//! `std::mem::discriminant(Value<InternerKey>)`. `ScalarRef` is a DIFFERENT
//! enum (6 variants vs `Value`'s 10) → hashing `ScalarRef` directly DIVERGES.
//!
//! We therefore materialise each indexed leaf to an owned `InnerValue` via
//! `RecordRef::materialize_at` and feed THAT through the UNCHANGED
//! `IndexRecordKey::with_values::<InnerValue>`. Byte-identical by
//! construction. NEVER hash `ScalarRef` for these keys.
//!
//! `materialize_at` (NOT `scalar_at`) is mandatory here: `scalar_at` returns
//! `None` for Dec/Big/containers, which would silently drop those records
//! from the index. `materialize_at` preserves any leaf.
//!
//! # Accepted limit — #61 InnerValue-elimination campaign
//!
//! The materialized `InnerValue` leaf fed to `IndexRecordKey::with_values`
//! (and the `Vec<(String, InnerValue)>` covering-projection blob in
//! `sorted_index_manager`) is a DELIBERATELY retained `InnerValue` anchor.
//! Persisted index posting hashes (`hash1`/`hash2`) and covering blobs are
//! on-disk and depend on `<Value<InternerKey> as Hash>`'s
//! `std::mem::discriminant`-prefixed byte stream (10 variants), which no
//! 6-variant `ScalarRef`/lens type can reproduce without divergence.
//! Eliminating it requires either a discriminant-stable hand-rolled hasher
//! proven against a frozen golden corpus, or an index-format version bump +
//! full O(N) rebuild migration that breaks storage byte-identity across
//! versions. Neither pays for itself: the record is already read via the byte
//! lens; only the transient indexed leaf is an `InnerValue`. Accepted limit
//! for the #61 InnerValue-elimination campaign.

use crate::legacy::index_info_item::IndexInfoItem;
use crate::legacy::index_record_key::IndexRecordKey;
use bytes::Bytes;
use shamir_types::core::interner::InternerKey;
use shamir_types::record_view::RecordRef;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use smallvec::SmallVec;

/// Extract the indexed leaves for a composite index from a record.
///
/// For each `IndexInfoItem` path: convert `&[u64]` → `&[InternerKey]` and
/// call `rec.materialize_at(path)`. If ANY path is missing (returns `None`)
/// the WHOLE record is skipped (returns `None`) — mirrors the legacy
/// all-or-nothing semantics of the old `extract_index_values`.
///
/// Uses `materialize_at` (NOT `scalar_at`) so Dec/Big/Map/List leaves are
/// preserved and stay indexed byte-identically to the legacy path.
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

/// Build the 25-byte index key from already-materialised leaves.
///
/// Feeds `&leaves` through the UNCHANGED `IndexRecordKey::with_values::<InnerValue>`
/// — the byte-identity anchor. The hashing boundary is exactly here: leaves
/// are owned `InnerValue`s, so `<InnerValue as Hash>::hash` is invoked
/// (matching the legacy discriminant + leaf bytes), never `ScalarRef`'s.
pub(super) fn build_index_key(
    is_unique: bool,
    name_interned: u64,
    leaves: &[InnerValue],
) -> IndexRecordKey {
    let leaf_refs: Vec<&InnerValue> = leaves.iter().collect();
    IndexRecordKey::new(is_unique, name_interned).with_values(&leaf_refs)
}

/// Compose the physical posting key:
/// `index_key (25b) || record_id (16b)` = 41 bytes.
pub(super) fn build_posting_key(index_key: &Bytes, record_id: &RecordId) -> Bytes {
    let mut k = Vec::with_capacity(index_key.len() + 16);
    k.extend_from_slice(index_key);
    k.extend_from_slice(record_id.as_bytes());
    Bytes::from(k)
}
