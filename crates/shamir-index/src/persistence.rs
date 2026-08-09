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
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use shamir_storage::types::{RecordKey, Store};
use shamir_tunables::store_defaults::MAINT_SCAN_BATCH;
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
    save_index2_metadata_with_pending(registry, info_store, None).await
}

/// Persist the index2 registry metadata, optionally including a `pending`
/// in-flight descriptor that is NOT yet in the live registry (F-50 Step 3b).
///
/// `create_index_v2` builds a backend's descriptor with `state = Building`
/// and needs that Building descriptor durably on disk BEFORE the backfill
/// runs (so a crash between the backfill and the final `Ready` persist is
/// detectable on restart). But the backend must NOT be inserted into the
/// live registry until after the backfill completes (the live
/// `index2_on_insert` hook can't route to an unregistered backend — see
/// `backfill_index2_backend`'s doc comment). This variant lets the caller
/// pass the in-flight Building descriptor as `pending` so the persisted set
/// is `all_descriptors() ∪ {pending}` without publishing the backend.
///
/// If `pending`'s id collides with an already-registered backend (defensive —
/// shouldn't happen in the normal create flow), the pending entry is dropped
/// in favor of the live registry's entry.
pub async fn save_index2_metadata_with_pending(
    registry: &crate::IndexRegistry,
    info_store: &Arc<dyn Store>,
    pending: Option<IndexDescriptor>,
) -> Result<(), shamir_storage::error::DbError> {
    let mut descriptors = registry.all_descriptors().await;
    if let Some(d) = pending {
        // Defensive de-dup: never write two descriptors with the same id
        // (the normal create flow is pre-register so this branch is a no-op,
        // but a caller that races a concurrent insert + pending with the
        // same id must not produce a malformed blob).
        if !descriptors.iter().any(|x| x.id == d.id) {
            descriptors.push(d);
        }
    }
    let p = PersistedIndexes {
        next_id: registry.peek_next_id(),
        descriptors,
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

// ============================================================================
// P0-3b (#988): DROP INDEX durable tombstone + crash recovery (index2)
// ============================================================================
//
// Mirrors #972's sorted-family and #959's base_index-family durable-tombstone
// pattern for the index2 family (fts / functional / vector).
//
// Sub-bug 3c (crash-resurrection): the OLD `drop_index2` had retire →
// sweep → persist ordering with NO durable tombstone. A crash between the
// sweep and the `save_index2_metadata` persist left the on-disk
// `PersistedIndexes` still listing the index as `Ready` while its postings
// were gone — on restart the planner would route queries to an index with
// zero postings, silently returning wrong (empty / missing) results. The
// durable tombstone (written BEFORE the sweep, cleared AFTER the persist)
// closes this: on restart, `TableManager::create`'s recovery hook sees the
// tombstone, removes the stale backend, sweeps its postings (idempotent),
// persists, and clears the tombstone.
//
// Sub-bug 3b (name-reuse ghost postings): an id in the tombstone set is
// rejected by `create_index_v2`'s namespace-reuse guard (resolving the
// tombstoned id's name via persisted metadata) until the tombstone clears.
//
// Tombstone shape: `Vec<(u32, String, Option<String>)>` (descriptor id, index name, op_id)
// serialized via bincode under `system:_m.idx.drop`. A separate key was chosen
// (mirroring #959's `idx_drop`/`uidx_drop` and #972's `sidx_drop`) to avoid touching
// `save_index2_metadata`/`load_index2_metadata`'s forward-compat fallback chain.
// An absent key or empty vec means "no in-progress drops".
//
// **CRITICAL** (the hard-won #959 collision lesson): `RecordId::system(name)`
// truncates `name` to 12 bytes. The key `"_m.idx.drop"` (11 bytes, no
// truncation) is verified distinct from the two existing index2 system keys:
//   `"_m.idx"`      → [0,0,0,0, 5f,6d,2e,69,64,78, 00,00,00,00,00,00]
//   `"_m.idx.lfv"`  → [0,0,0,0, 5f,6d,2e,69,64,78, 2e,6c,66,76,00,00]
//   `"_m.idx.drop"` → [0,0,0,0, 5f,6d,2e,69,64,78, 2e,64,72,6f,70,00]
// They share the first 10 bytes (`\0\0\0\0_m.idx`) but diverge at byte 10
// (0x00 vs 0x2e) and again at byte 11 (0x6c vs 0x64) — NO collision.
//
// #1051: the tombstone now carries the index name and operation ID minted at dispatch time
// (the `String` and `Option<String>` components). This enables recovery to:
// 1. Use the id for registry operations (remove_by_id, sweep)
// 2. Use the name to write the correct DdlOpStatus
// 3. Use the op_id to ensure clients can poll the exact operation they triggered

/// System key for the index2 DROP tombstone (`Vec<(u32, String, Option<String>)>` of id + name + op_id).
fn meta_key_indexes_drop() -> RecordId {
    RecordId::system("_m.idx.drop")
}

/// Load the persisted index2 DROP tombstone. Returns an empty `Vec` if the
/// key is absent (`NotFound`) or contains an empty vec — both mean "no
/// in-progress drops". Mirrors #972's `load_dropping_sorted`.
///
/// #1051 backward compatibility: tries to deserialize as `Vec<(u32, String, Option<String>)>`
/// first (new format with name and op_id). If that fails, falls back to `Vec<u32>` (old format
/// without name/op_id), treating all entries as having empty name and `op_id = None`.
pub async fn load_dropping_index2(
    info_store: &Arc<dyn Store>,
) -> Result<Vec<(u32, String, Option<String>)>, shamir_storage::error::DbError> {
    let key = meta_key_indexes_drop().to_bytes();
    match info_store.get(key.into()).await {
        Ok(bytes) => {
            if bytes.is_empty() {
                return Ok(Vec::new());
            }
            // Try new format first: Vec<(u32, String, Option<String>)>
            match bincode::deserialize::<Vec<(u32, String, Option<String>)>>(&bytes) {
                Ok(new_format) => Ok(new_format),
                Err(_) => {
                    // Fall back to old format: Vec<u32> (pre-#1051)
                    let old_format: Vec<u32> = bincode::deserialize(&bytes).map_err(|e| {
                        shamir_storage::error::DbError::Codec(format!(
                            "system:_m.idx.drop decode failed: {e}"
                        ))
                    })?;
                    // Convert to new format with empty name and op_id = None
                    // (we can't recover the name in this case, so status writes will be skipped)
                    Ok(old_format
                        .into_iter()
                        .map(|id| (id, String::new(), None))
                        .collect())
                }
            }
        }
        Err(shamir_storage::error::DbError::NotFound(_)) => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// Add `id` to the persisted index2 DROP tombstone. Loads the current set,
/// appends the entry, and writes it back. MUST be called BEFORE the sweep in
/// `drop_index2` so a crash at any subsequent point is recoverable.
///
/// If the persist fails, the on-disk tombstone is unchanged — the caller
/// (`drop_index2`) propagates the error and does NOT proceed with the drop.
/// Mirrors #972's `add_to_dropping_sorted` (adapted for the free-function
/// shape — `IndexRegistry` does not own an `info_store`).
///
/// #1051: accepts `name` and `op_id` minted at dispatch time. `op_id = None` means
/// the caller does not have an op_id (e.g., a non-DDL path), and the tombstone entry
/// will have `op_id = None` (backward compatible with pre-#1051 format).
pub async fn add_to_dropping_index2(
    id: u32,
    name: String,
    op_id: Option<String>,
    info_store: &Arc<dyn Store>,
) -> Result<(), shamir_storage::error::DbError> {
    let mut current = load_dropping_index2(info_store).await?;
    if !current.iter().any(|&(existing_id, _, _)| existing_id == id) {
        current.push((id, name, op_id));
    }
    let key = meta_key_indexes_drop().to_bytes();
    let bytes = bincode::serialize(&current)
        .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
    info_store.set(key.into(), Bytes::from(bytes)).await?;
    Ok(())
}

/// Remove `id` from the persisted index2 DROP tombstone. Loads the current
/// set, removes the id, and writes it back. MUST be called AFTER
/// `save_index2_metadata` (the reduced metadata must be durable first).
/// Mirrors #972's `clear_from_dropping_sorted`.
pub async fn clear_from_dropping_index2(
    id: u32,
    info_store: &Arc<dyn Store>,
) -> Result<(), shamir_storage::error::DbError> {
    let mut current = load_dropping_index2(info_store).await?;
    current.retain(|&(existing_id, _, _)| existing_id != id);
    let key = meta_key_indexes_drop().to_bytes();
    let bytes = bincode::serialize(&current)
        .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
    info_store.set(key.into(), Bytes::from(bytes)).await?;
    Ok(())
}

/// Sweep all posting entries for the given index2 descriptor id from
/// `info_store`. Postings are keyed by `[index_id: u32 LE][type_tag]...`
/// (see `posting_layout`), so a 4-byte prefix scan collects every entry.
/// Removing already-removed keys is a no-op on every `Store` backend, so
/// this is safe to call idempotently. Extracted as a free function so the
/// recovery path can re-run it without holding a backend `Arc`.
///
/// Note: `VectorBackend::drop_all` is currently a no-op (the HNSW graph
/// lives in memory and dies with the process); this sweep finds no
/// posting-key entries for vector ids, which is consistent with that.
/// Mirrors #972's `sweep_sorted_postings`.
pub async fn sweep_index2_postings_by_id(
    id: u32,
    info_store: &Arc<dyn Store>,
) -> Result<(), shamir_storage::error::DbError> {
    let prefix = Bytes::copy_from_slice(&id.to_le_bytes());
    let stream = info_store.scan_prefix_stream(prefix, MAINT_SCAN_BATCH);
    futures::pin_mut!(stream);
    let mut to_drop: Vec<RecordKey> = Vec::new();
    while let Some(batch) = stream.next().await {
        for (k, _) in batch? {
            to_drop.push(k);
        }
    }
    if !to_drop.is_empty() {
        let _ = info_store.remove_many(to_drop).await?;
    }
    Ok(())
}
