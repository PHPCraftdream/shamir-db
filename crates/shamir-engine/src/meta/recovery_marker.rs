//! Durable recovery markers for the upcoming transactional layer.
//!
//! Two `u64` markers persisted under [`MetaKey::LastCommittedVersion`]
//! and [`MetaKey::NextTxId`] in the repo's info_store. Both go through
//! [`MetaEnvelope`] so future schema migrations dispatch on the
//! envelope's version field.
//!
//! Currently NOT wired into any production write path — used only by
//! tests. Stage 2 (`RepoTxGate`) will call these helpers on commit /
//! periodic snapshot and on repo open.

use crate::meta::{MetaEnvelope, MetaError, MetaKey};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::Store;
use std::sync::Arc;

fn convert(err: MetaError) -> DbError {
    DbError::Internal(format!("recovery_marker codec: {err}"))
}

/// Load the most recent committed MVCC version from the info_store.
/// Returns `Ok(None)` if the marker has never been written (fresh
/// repo). Returns `Err` only on real storage / decode failures.
pub async fn load_last_committed(info_store: &Arc<dyn Store>) -> DbResult<Option<u64>> {
    load_u64(info_store, MetaKey::LastCommittedVersion).await
}

/// Persist the last committed MVCC version. Called by `RepoTxGate`
/// during the publish phase of every tx commit.
pub async fn save_last_committed(info_store: &Arc<dyn Store>, version: u64) -> DbResult<()> {
    save_u64(info_store, MetaKey::LastCommittedVersion, version).await
}

/// Load the persisted snapshot of `next_tx_id`. Returns `Ok(None)`
/// if never written. On repo open the gate seeds its counter from
/// `max(this, max(WAL active txn_id))` so issued ids never collide.
pub async fn load_next_tx_id_snapshot(info_store: &Arc<dyn Store>) -> DbResult<Option<u64>> {
    load_u64(info_store, MetaKey::NextTxId).await
}

/// Persist the current `next_tx_id` periodically (e.g. every N
/// commits) so recovery doesn't have to scan the full active-WAL
/// prefix to seed the counter.
pub async fn save_next_tx_id_snapshot(info_store: &Arc<dyn Store>, value: u64) -> DbResult<()> {
    save_u64(info_store, MetaKey::NextTxId, value).await
}

pub(crate) async fn load_u64(info_store: &Arc<dyn Store>, key: MetaKey) -> DbResult<Option<u64>> {
    match info_store.get(key.as_record_id().to_bytes().into()).await {
        Ok(bytes) => {
            let val: u64 = MetaEnvelope::open(&bytes).map_err(convert)?;
            Ok(Some(val))
        }
        Err(DbError::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

pub(crate) async fn save_u64(
    info_store: &Arc<dyn Store>,
    key: MetaKey,
    value: u64,
) -> DbResult<()> {
    let envelope = MetaEnvelope::new(value);
    let bytes = envelope.encode().map_err(convert)?;
    info_store
        .set(
            key.as_record_id().to_bytes().into(),
            bytes::Bytes::from(bytes),
        )
        .await
        .map(|_| ())
}
