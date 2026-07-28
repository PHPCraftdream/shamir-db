//! Persistence for index2 registry — save/load `IndexDescriptor`s
//! via `MetaEnvelope` to `__meta__/indexes`.
//!
//! # Legacy index format version (S9)
//!
//! `LEGACY_INDEX_FORMAT_VERSION` tracks the on-disk format of the legacy
//! hash/unique/sorted index postings. When the engine opens a table whose
//! stored version is less than the current constant, it MUST trigger a full
//! O(N) rebuild (drop old postings, re-index every record from the data
//! store via the doctor's `repair`-style machinery). This is a one-time
//! reindex per table per version bump.
//!
//! Version history:
//!   1 — original format: `<Value<InternerKey> as Hash>` with
//!       `std::mem::discriminant` tags; covering projection as
//!       `Vec<(String, InnerValue)>`.
//!   2 — S9 lens-native format: stable u8 discriminant tags via
//!       `hash_scalar_ref`/`hash_inner_value`; covering projection as
//!       `Vec<(String, QueryValue)>` (scalar-only, wire-compat with
//!       InnerValue decode).

use crate::descriptor::IndexDescriptor;
use crate::{MetaEnvelope, MetaError};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use shamir_storage::types::Store;
use shamir_types::types::record_id::RecordId;
use std::sync::Arc;

/// Current legacy index posting format version. Bump this whenever
/// the posting-key hash scheme or covering-projection blob format
/// changes in a way that is NOT backward-compatible with existing
/// on-disk data. The engine checks this on open and triggers a
/// rebuild when the stored version is older.
pub const LEGACY_INDEX_FORMAT_VERSION: u32 = 2;

// The meta key tag "_m.idx" is byte-identical to MetaKey::Indexes.tag() in the engine.
fn meta_key_indexes() -> RecordId {
    RecordId::system("_m.idx")
}

/// System key for the legacy index format version marker.
fn meta_key_legacy_index_version() -> RecordId {
    RecordId::system("_m.idx.lfv")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedIndexes {
    pub next_id: u32,
    pub descriptors: Vec<IndexDescriptor>,
}

pub async fn save_index2_metadata(
    registry: &crate::IndexRegistry,
    info_store: &Arc<dyn Store>,
) -> Result<(), shamir_storage::error::DbError> {
    let p = PersistedIndexes {
        next_id: registry.peek_next_id(),
        descriptors: registry.all_descriptors().await,
    };
    let envelope = MetaEnvelope::new(p);
    let bytes = envelope
        .encode()
        .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?;
    let key = meta_key_indexes();
    info_store
        .set(key.to_bytes().into(), Bytes::from(bytes))
        .await
        .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?;
    Ok(())
}

pub async fn load_index2_metadata(
    info_store: &Arc<dyn Store>,
) -> Result<Option<PersistedIndexes>, shamir_storage::error::DbError> {
    let key = meta_key_indexes();
    let bytes = match info_store.get(key.to_bytes().into()).await {
        Ok(bytes) => bytes,
        Err(shamir_storage::error::DbError::NotFound(_)) => return Ok(None),
        Err(e) => return Err(e),
    };
    Ok(Some(decode_persisted_indexes(&bytes)?))
}

/// Decode the `__meta__/indexes` blob into the current [`PersistedIndexes`],
/// with a forward-compat fallback for blobs written BEFORE the `state`
/// field existed (F-50 Step 3a).
///
/// bincode 1.3.3 does NOT honour `#[serde(default)]` for a NEW trailing
/// field — a pre-`state` blob fails to decode as the current shape with
/// `io error: unexpected end of file` (proven by the F-50 Step 3a
/// round-trip test, `tests/index_state_compat_tests.rs`). So the current
/// shape is tried first; on a decode failure the pre-`state` shadow shape
/// is tried and each descriptor is lifted to `state = Ready` (every
/// pre-`state` persisted index was fully built — a `Building` index could
/// never have been persisted before the `state` field existed). Any other
/// failure (bad magic, unsupported envelope version, or a blob that
/// decodes as NEITHER shape — genuine corruption) is surfaced as an error.
pub(crate) fn decode_persisted_indexes(
    bytes: &[u8],
) -> Result<PersistedIndexes, shamir_storage::error::DbError> {
    match MetaEnvelope::<PersistedIndexes>::open(bytes) {
        Ok(p) => Ok(p),
        Err(MetaError::Decode(new_err)) => {
            // Possible pre-`state` blob — try the legacy shadow shape.
            match MetaEnvelope::<forward_compat::PersistedIndexesNoState>::open(bytes) {
                Ok(legacy) => {
                    log::warn!(
                        "index2 metadata: decoded with pre-`state` legacy fallback \
                         ({} descriptor(s) lifted to state=Ready). \
                         New-shape decode error: {}",
                        legacy.descriptors.len(),
                        new_err
                    );
                    Ok(PersistedIndexes::from(legacy))
                }
                Err(legacy_err) => {
                    // Decodes as neither shape — genuine corruption.
                    Err(shamir_storage::error::DbError::Internal(format!(
                        "index2 metadata decode failed (new shape: {new_err}; \
                         legacy shape: {legacy_err})"
                    )))
                }
            }
        }
        Err(e) => Err(shamir_storage::error::DbError::Internal(e.to_string())),
    }
}

/// Pre-`state` on-disk shadow shapes used ONLY by the forward-compat
/// fallback in [`load_index2_metadata`]. These mirror the exact field
/// order/types of `IndexDescriptor`/`PersistedIndexes` as they existed
/// BEFORE the `state: IndexState` field was added (F-50 Step 3a), so
/// genuinely old on-disk bytes can be decoded and lifted to the current
/// shape. Write-side always emits the current shape; these are read-only.
mod forward_compat {
    use crate::kind::IndexKind;
    use serde::{Deserialize, Serialize};
    use smallvec::SmallVec;

    /// Pre-`state` `IndexDescriptor` shadow.
    #[derive(Debug, Serialize, Deserialize)]
    pub(in crate::persistence) struct IndexDescriptorNoState {
        pub id: u32,
        pub name: String,
        pub name_interned: u64,
        pub paths: SmallVec<[Vec<u64>; 2]>,
        pub kind: IndexKind,
        pub created_at_nanos: u64,
        #[serde(default)]
        pub options: Vec<u8>,
    }

    /// Pre-`state` `PersistedIndexes` shadow.
    #[derive(Debug, Serialize, Deserialize)]
    pub(in crate::persistence) struct PersistedIndexesNoState {
        pub next_id: u32,
        pub descriptors: Vec<IndexDescriptorNoState>,
    }
}

impl From<forward_compat::PersistedIndexesNoState> for PersistedIndexes {
    fn from(legacy: forward_compat::PersistedIndexesNoState) -> Self {
        PersistedIndexes {
            next_id: legacy.next_id,
            descriptors: legacy
                .descriptors
                .into_iter()
                .map(|d| IndexDescriptor {
                    id: d.id,
                    name: d.name,
                    name_interned: d.name_interned,
                    paths: d.paths,
                    kind: d.kind,
                    created_at_nanos: d.created_at_nanos,
                    options: d.options,
                    // Every pre-`state` persisted index was fully built;
                    // a `Building` index could not have been persisted
                    // before this field existed.
                    state: crate::state::IndexState::default(),
                })
                .collect(),
        }
    }
}

// ============================================================================
// Legacy index format version (S9)
// ============================================================================

/// Persist the current `LEGACY_INDEX_FORMAT_VERSION` to the info store.
/// Called after a successful index rebuild or on first index creation.
pub async fn save_legacy_index_version(
    info_store: &Arc<dyn Store>,
) -> Result<(), shamir_storage::error::DbError> {
    let key = meta_key_legacy_index_version();
    let bytes = LEGACY_INDEX_FORMAT_VERSION.to_le_bytes();
    info_store
        .set(key.to_bytes().into(), Bytes::from(bytes.to_vec()))
        .await
        .map(|_| ())
}

/// Load the stored legacy index format version. Returns `0` if no
/// version marker exists (pre-S9 data — always needs rebuild).
pub async fn load_legacy_index_version(
    info_store: &Arc<dyn Store>,
) -> Result<u32, shamir_storage::error::DbError> {
    let key = meta_key_legacy_index_version();
    match info_store.get(key.to_bytes().into()).await {
        Ok(bytes) => {
            if bytes.len() >= 4 {
                let arr: [u8; 4] = bytes[..4].try_into().unwrap_or([0; 4]);
                Ok(u32::from_le_bytes(arr))
            } else {
                Ok(0)
            }
        }
        Err(shamir_storage::error::DbError::NotFound(_)) => Ok(0),
        Err(e) => Err(e),
    }
}

/// Check whether the legacy index postings need a rebuild (stored version
/// is older than `LEGACY_INDEX_FORMAT_VERSION`).
pub async fn legacy_indexes_need_rebuild(
    info_store: &Arc<dyn Store>,
) -> Result<bool, shamir_storage::error::DbError> {
    let stored = load_legacy_index_version(info_store).await?;
    Ok(stored < LEGACY_INDEX_FORMAT_VERSION)
}
