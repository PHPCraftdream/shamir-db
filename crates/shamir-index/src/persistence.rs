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
use crate::MetaEnvelope;
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
    match info_store.get(key.to_bytes().into()).await {
        Ok(bytes) => {
            let p: PersistedIndexes = MetaEnvelope::open(&bytes)
                .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?;
            Ok(Some(p))
        }
        Err(shamir_storage::error::DbError::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
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
