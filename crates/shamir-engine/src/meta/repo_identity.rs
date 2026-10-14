//! Durable repository incarnation and restore receipt (SEED-1).
//! Callers serialize load/create/bump with the repository's shared marker mutex.
//!
//! Also hosts the lifecycle record for `completed_reseed` speculative
//! replication serving: while a repo serves completed-but-not-yet-covered
//! versions, an unclean end of the process conservatively costs every
//! follower a re-seed. The record is `Armed` before the first speculative
//! page is served (arm + flush strictly precede any served byte) and becomes
//! `Clean` only at a clean shutdown, AFTER journal horizon, allocation
//! settle, data buffers and WAL evidence are all durable — so a surviving
//! `Clean` certificate implies all of them. On open, an `Armed` record with
//! no later `Clean` certificate triggers the history-epoch bump (followers
//! must re-seed). Codec mirrors the identity record below: versioned-tuple
//! `MetaEnvelope` over one `MetaKey` store value, `set` then `flush`.

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
        // Negative-control seam (see `SKIP_BUMP_FLUSH_FOR_TEST`): constant-true
        // in production, branch elided under cfg!(test) when unset.
        let flush =
            !(cfg!(test) && SKIP_BUMP_FLUSH_FOR_TEST.load(std::sync::atomic::Ordering::Acquire));
        Self::save_record_inner(store, identity, operation.or(receipt), flush).await?;
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
        Self::save_record_inner(store, identity, receipt, true).await
    }

    /// Inner save with a caller-controlled flush: `save_record` (public
    /// behaviour) always flushes; only the `bump` path may skip it, and only
    /// via the negative-control seam below.
    async fn save_record_inner(
        store: &Arc<dyn Store>,
        identity: Self,
        receipt: Option<[u8; 16]>,
        flush: bool,
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
        if flush {
            store.flush().await?;
        }
        Ok(())
    }
}

/// Negative-control seam for tests: when set, the open-time recovery hook in
/// `RepoInstance::note_unclean_open_bump` skips the unclean history-epoch
/// bump (mirrors `SKIP_S_CLAMP_FOR_TEST` in `replication_bound.rs`). Always
/// compiled; production reads are gated by a constant-false `cfg!(test)` so
/// the branch is elided and no `unused` warning fires on any platform.
pub(crate) static SKIP_UNCLEAN_EPOCH_BUMP_FOR_TEST: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Negative-control seam for tests ONLY: when set, `ensure_armed` performs
/// the store `set` but SKIPS the `flush`, so an unclean process end loses the
/// Armed record. Exists solely so the crash tests can prove the arming flush
/// is load-bearing (without it, reopening finds no record and no bump fires).
/// Always compiled; the read site is gated by a constant-false `cfg!(test)`
/// so the branch is elided in production and no `unused` warning fires on any
/// platform. nextest runs each test in its own process, so setting it cannot
/// race any other test.
pub(crate) static SKIP_ARMING_FLUSH_FOR_TEST: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Negative-control seam for tests ONLY: when set, `mark_clean` performs the
/// store `set` but SKIPS the `flush`, so an unclean process end loses the
/// Clean certificate. Exists solely so the crash tests can prove the
/// clean-certificate flush is load-bearing (without it, a "Clean" shutdown
/// still costs the reopened repo an epoch bump). Same cfg!(test)/nextest
/// story as `SKIP_ARMING_FLUSH_FOR_TEST`.
pub(crate) static SKIP_CLEAN_FLUSH_FOR_TEST: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Negative-control seam for tests ONLY: when set, the actual-bump path of
/// [`RepoIdentity::bump`] skips the flush inside the record save (the
/// idempotent-receipt early return is unaffected), so the bumped epoch can be
/// lost by an unclean process end. Exists solely so the crash tests can prove
/// a torn/lost bump converges (the next open re-derives and re-bumps) instead
/// of corrupting. Same cfg!(test)/nextest story as `SKIP_ARMING_FLUSH_FOR_TEST`.
pub(crate) static SKIP_BUMP_FLUSH_FOR_TEST: std::sync::atomic::AtomicBool =
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
/// hook must bump the history epoch iff the loaded record is `Armed` (no
/// valid clean certificate after it). The CURRENT serve mode is deliberately
/// NOT consulted (review finding 2a): an Armed obligation must survive a
/// configuration change — if a repo served speculatively, was switched to
/// `durable`, and then restarts uncleanly, the served tail is gone and the
/// epoch MUST still bump (under durable serving an old-epoch follower
/// bookmark is inert, so fencing requires the bump, not the mode).
/// `None` (no record) means nothing was ever served speculatively.
pub(crate) fn requires_unclean_bump(record: Option<LifecycleState>) -> bool {
    record == Some(LifecycleState::Armed)
}

async fn load_lifecycle_record(store: &Arc<dyn Store>) -> DbResult<Option<LifecycleState>> {
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
async fn save_lifecycle_record(store: &Arc<dyn Store>, state: LifecycleState) -> DbResult<()> {
    save_lifecycle_record_inner(store, state, true).await
}

/// Inner save with a caller-controlled flush; only the negative-control seams
/// below may pass `flush: false`.
async fn save_lifecycle_record_inner(
    store: &Arc<dyn Store>,
    state: LifecycleState,
    flush: bool,
) -> DbResult<()> {
    let bytes = MetaEnvelope::new((1u16, state))
        .encode()
        .map_err(|e| DbError::Storage(format!("lifecycle record codec: {e}")))?;
    store
        .set(
            RecordKey::from_slice(MetaKey::LifecycleRecord.as_record_id().as_bytes()),
            bytes::Bytes::from(bytes),
        )
        .await?;
    if flush {
        store.flush().await?;
    }
    Ok(())
}

/// Idempotent arm: `Some(Armed)` already → no write; otherwise write Armed
/// + flush. Writing Armed over a stale `Clean` is CORRECT: a new
///   speculative-serve period re-arms the obligation.
pub(crate) async fn ensure_armed(store: &Arc<dyn Store>) -> DbResult<()> {
    if load_lifecycle_record(store).await? == Some(LifecycleState::Armed) {
        return Ok(());
    }
    // Negative-control seam (see `SKIP_ARMING_FLUSH_FOR_TEST`): constant-true
    // in production, branch elided under cfg!(test) when unset.
    let flush =
        !(cfg!(test) && SKIP_ARMING_FLUSH_FOR_TEST.load(std::sync::atomic::Ordering::Acquire));
    save_lifecycle_record_inner(store, LifecycleState::Armed, flush).await
}

/// Unconditional clean certificate overwrite + flush. Must be called only
/// after the clean-shutdown steps it certifies (see module doc).
pub(crate) async fn mark_clean(store: &Arc<dyn Store>) -> DbResult<()> {
    // Negative-control seam (see `SKIP_CLEAN_FLUSH_FOR_TEST`): constant-true
    // in production, branch elided under cfg!(test) when unset.
    let flush =
        !(cfg!(test) && SKIP_CLEAN_FLUSH_FOR_TEST.load(std::sync::atomic::Ordering::Acquire));
    save_lifecycle_record_inner(store, LifecycleState::Clean, flush).await
}

/// Read the current state; `None` if no record was ever written.
pub(crate) async fn load_state(store: &Arc<dyn Store>) -> DbResult<Option<LifecycleState>> {
    load_lifecycle_record(store).await
}
