//! Durable repository incarnation and restore receipt (SEED-1).
//! Callers serialize load/create/bump with the repository's shared marker mutex.

use rand::TryRngCore;
use serde::{Deserialize, Serialize};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{RecordKey, Store};
use std::sync::Arc;

use crate::meta::{MetaEnvelope, MetaKey};

/// S5c-X PR7 (leader-side honesty): the lineage id identifies a DURABLE
/// incarnation of the repository. `in_memory` and `hybrid` repositories keep
/// `__tx__` (and therefore this record) in RAM only (`repo_types.rs`'s
/// ephemeral-store routing), so every process start mints a NEW random
/// lineage at epoch 0 — a follower's identity fence then correctly treats the
/// restarted leader as a different history instead of silently continuing.
/// This is deliberate: a NEW lineage (not an epoch bump) is the honest SEED-1
/// reading, because nothing of the previous incarnation survives. Durable
/// (`fjall`) repositories keep their identity across restarts unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoIdentity {
    pub lineage_id: [u8; 16],
    pub history_epoch: u64,
}

impl RepoIdentity {
    /// Public identifiers, not credentials. rand 0.9 OsRng uses OS entropy;
    /// entropy failures propagate rather than minting a predictable incarnation.
    pub fn new_lineage_id() -> DbResult<[u8; 16]> {
        let mut id = [0; 16];
        rand::rngs::OsRng
            .try_fill_bytes(&mut id)
            .map_err(|e| DbError::Storage(format!("repository identity entropy: {e}")))?;
        Ok(id)
    }

    pub(crate) async fn load_or_create(store: &Arc<dyn Store>) -> DbResult<Self> {
        let (identity, _) = Self::load_record(store).await?;
        // Also flush an existing record: a preceding cancelled/failed create
        // may have completed set but not flush. No caller acknowledges it early.
        store.flush().await?;
        Ok(identity)
    }

    pub(crate) async fn bump(
        store: &Arc<dyn Store>,
        operation: Option<[u8; 16]>,
    ) -> DbResult<Self> {
        let (mut identity, receipt) = Self::load_record(store).await?;
        if operation.is_some() && operation == receipt {
            store.flush().await?;
            return Ok(identity);
        }
        identity.history_epoch = identity
            .history_epoch
            .checked_add(1)
            .ok_or_else(|| DbError::Storage("repository history epoch overflow".into()))?;
        Self::save_record(store, identity, operation.or(receipt)).await?;
        Ok(identity)
    }

    async fn load_record(store: &Arc<dyn Store>) -> DbResult<(Self, Option<[u8; 16]>)> {
        let key = RecordKey::from_slice(MetaKey::RepoIdentity.as_record_id().as_bytes());
        match store.get(key).await {
            Ok(bytes) => {
                if bytes.len() != 41 && bytes.len() != 57 {
                    return Err(DbError::Storage(
                        "invalid repository identity length".into(),
                    ));
                }
                let (version, identity, receipt): (u16, Self, Option<[u8; 16]>) =
                    MetaEnvelope::open(&bytes)
                        .map_err(|e| DbError::Storage(format!("repository identity codec: {e}")))?;
                if version != 1 {
                    return Err(DbError::Storage(format!(
                        "unsupported repository identity version {version}"
                    )));
                }
                // Fixed-width bincode envelope: 14-byte header, 27-byte
                // payload without receipt, 43-byte payload with receipt.
                // Reject trailing bytes (bincode's default decoder permits them).
                let expected_length = if receipt.is_some() { 57 } else { 41 };
                if bytes.len() != expected_length {
                    return Err(DbError::Storage(
                        "invalid repository identity length".into(),
                    ));
                }
                Ok((identity, receipt))
            }
            Err(DbError::NotFound(_)) => {
                let identity = Self {
                    lineage_id: Self::new_lineage_id()?,
                    history_epoch: 0,
                };
                Self::save_record(store, identity, None).await?;
                Ok((identity, None))
            }
            Err(e) => Err(e),
        }
    }

    pub(crate) async fn save_record(
        store: &Arc<dyn Store>,
        identity: Self,
        receipt: Option<[u8; 16]>,
    ) -> DbResult<()> {
        // One atomic store value includes the bump AND its operation receipt.
        let bytes = MetaEnvelope::new((1u16, identity, receipt))
            .encode()
            .map_err(|e| DbError::Storage(format!("repository identity codec: {e}")))?;
        store
            .set(
                RecordKey::from_slice(MetaKey::RepoIdentity.as_record_id().as_bytes()),
                bytes::Bytes::from(bytes),
            )
            .await?;
        store.flush().await
    }
}
