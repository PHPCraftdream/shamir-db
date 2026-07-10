//! Persistence for per-table validator bindings — save/load
//! `PersistedValidators` via `MetaEnvelope` to `__meta__/validators`.
//!
//! Mirrors `crate::index2::persistence` exactly.

use crate::meta::MetaEnvelope;
use crate::validator::PersistedValidators;
use bytes::Bytes;
use shamir_storage::error::DbError;
use shamir_storage::types::Store;
use std::sync::Arc;

/// Persist the given validator bindings into the table's info-twin
/// under `MetaKey::Validators`.
pub async fn save_validators_metadata(
    bindings: &[crate::validator::ValidatorBinding],
    info_store: &Arc<dyn Store>,
) -> Result<(), DbError> {
    let p = PersistedValidators {
        bindings: bindings.to_vec(),
    };
    let envelope = MetaEnvelope::new(p);
    let bytes = envelope
        .encode()
        .map_err(|e| DbError::Internal(e.to_string()))?;
    let key = crate::meta::MetaKey::Validators.as_record_id();
    info_store
        .set(key.to_bytes().into(), Bytes::from(bytes))
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?;
    Ok(())
}

/// Load the persisted validator bindings from the table's info-twin.
/// Returns `Ok(None)` when no bindings have been saved yet.
pub async fn load_validators_metadata(
    info_store: &Arc<dyn Store>,
) -> Result<Option<PersistedValidators>, DbError> {
    let key = crate::meta::MetaKey::Validators.as_record_id();
    match info_store.get(key.to_bytes().into()).await {
        Ok(bytes) => {
            let p: PersistedValidators =
                MetaEnvelope::open(&bytes).map_err(|e| DbError::Internal(e.to_string()))?;
            Ok(Some(p))
        }
        Err(DbError::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}
