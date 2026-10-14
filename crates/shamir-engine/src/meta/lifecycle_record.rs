//! Lifecycle record for `completed_reseed` speculative replication serving.
//!
//! While a repo serves completed-but-not-yet-covered versions, an unclean
//! end of the process conservatively costs every follower a re-seed. The
//! record is `Armed` before the first speculative page is served (arm +
//! flush strictly precede any served byte) and becomes `Clean` only at a
//! clean shutdown, AFTER journal horizon, allocation settle, data buffers
//! and WAL evidence are all durable — so a surviving `Clean` certificate
//! implies all of them. On open, an `Armed` record with no later `Clean`
//! certificate triggers the history-epoch bump (followers must re-seed).
//! Codec mirrors `meta/repo_identity.rs`: versioned-tuple `MetaEnvelope`
//! over one `MetaKey` store value, `set` then `flush`.

use serde::{Deserialize, Serialize};
use shamir_query_types::ReplicationServe;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{RecordKey, Store};
use std::sync::Arc;

use crate::meta::{MetaEnvelope, MetaKey};

/// Negative-control seam for tests: when set, the open-time recovery hook in
/// `RepoInstance::note_unclean_open_bump` skips the unclean history-epoch
/// bump (mirrors `SKIP_S_CLAMP_FOR_TEST` in `replication_bound.rs`). Always
/// compiled; production reads are gated by a constant-false `cfg!(test)` so
/// the branch is elided and no `unused` warning fires on any platform.
pub(crate) static SKIP_UNCLEAN_EPOCH_BUMP_FOR_TEST: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Arm/Clean certificate state of the speculative-serve period.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LifecycleState {
    /// Speculative serving may have happened; an unclean end costs a re-seed.
    Armed,
    /// Clean shutdown certificate: journal + allocation + data + WAL all
    /// durable; no re-seed obligation.
    Clean,
}

/// Pure decision helper (unit-testable without IO): the open-time recovery
/// hook must bump the history epoch iff the repo serves `CompletedReseed`
/// AND the loaded record is `Armed` (no valid clean certificate after it).
/// `None` (no record) means nothing was ever served speculatively.
pub(crate) fn requires_unclean_bump(
    mode: ReplicationServe,
    record: Option<LifecycleState>,
) -> bool {
    mode == ReplicationServe::CompletedReseed && record == Some(LifecycleState::Armed)
}

async fn load_record(store: &Arc<dyn Store>) -> DbResult<Option<LifecycleState>> {
    let key = RecordKey::from_slice(MetaKey::LifecycleRecord.as_record_id().as_bytes());
    match store.get(key).await {
        Ok(bytes) => {
            let (version, state): (u16, LifecycleState) = MetaEnvelope::open(&bytes)
                .map_err(|e| DbError::Storage(format!("lifecycle record codec: {e}")))?;
            if version != 1 {
                return Err(DbError::Storage(format!(
                    "unsupported lifecycle record version {version}"
                )));
            }
            Ok(Some(state))
        }
        Err(DbError::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// One atomic store value; `set` then `flush` so the state never survives a
/// crash half-written.
async fn save_record(store: &Arc<dyn Store>, state: LifecycleState) -> DbResult<()> {
    let bytes = MetaEnvelope::new((1u16, state))
        .encode()
        .map_err(|e| DbError::Storage(format!("lifecycle record codec: {e}")))?;
    store
        .set(
            RecordKey::from_slice(MetaKey::LifecycleRecord.as_record_id().as_bytes()),
            bytes::Bytes::from(bytes),
        )
        .await?;
    store.flush().await
}

/// Idempotent arm: `Some(Armed)` already → no write; otherwise write Armed
/// + flush. Writing Armed over a stale `Clean` is CORRECT: a new
/// speculative-serve period re-arms the obligation.
pub(crate) async fn ensure_armed(store: &Arc<dyn Store>) -> DbResult<()> {
    if load_record(store).await? == Some(LifecycleState::Armed) {
        return Ok(());
    }
    save_record(store, LifecycleState::Armed).await
}

/// Unconditional clean certificate overwrite + flush. Must be called only
/// after the clean-shutdown steps it certifies (see module doc).
pub(crate) async fn mark_clean(store: &Arc<dyn Store>) -> DbResult<()> {
    save_record(store, LifecycleState::Clean).await
}

/// Read the current state; `None` if no record was ever written.
pub(crate) async fn load_state(store: &Arc<dyn Store>) -> DbResult<Option<LifecycleState>> {
    load_record(store).await
}
