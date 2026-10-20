//! Mandatory per-response identity decision; seed receipts also pin schema.

use std::sync::Arc;

use shamir_db::engine::meta::repl::bookmark_v3::load_bookmark_v3;
use shamir_db::engine::meta::repl_bookmark::load_replication_bookmark;
use shamir_db::engine::meta::repo_identity::RepoIdentity;
use shamir_db::engine::repo::SeedReceipt;
use shamir_db::storage::types::Store;
use shamir_db::{SeedRefusal, ShamirDb};
use shamir_query_types::wire::repl::ReplResponse;

use super::recorded::{self, FenceRecord};
use crate::replication::error::ReplError;

/// Map a storage error to a replication error WITHOUT flattening a typed
/// follower-state refusal into a retryable string (PR6b-3): the refusal
/// stays machine-readable and terminal; everything else keeps the
/// `Bookmark` string form.
///
/// Negative-control seam for tests: when set, the supervisor skips the
/// `resync_required` quarantine after an identity/epoch fence trip (mirrors
/// `SKIP_S_CLAMP_FOR_TEST` in the engine's `replication_bound.rs`). Always
/// compiled; the supervisor reads it behind `cfg!(test)` so the branch is
/// elided in production and the static is never dead on any platform.
pub(crate) static SKIP_FENCE_QUARANTINE_FOR_TEST: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub(crate) fn storage_error(error: shamir_db::storage::error::DbError) -> ReplError {
    match error {
        shamir_db::storage::error::DbError::FollowerStateRefused { reason } => {
            ReplError::FollowerStateRefused { reason }
        }
        other => ReplError::Bookmark(other.to_string()),
    }
}

/// Re-establish durability once per loop start, before any leader contact.
/// A failed/cancelled set+flush may be readable without having reached disk.
/// Checks return on persistence errors, so no later response in that loop can
/// trust such a write; a restarted loop must pass this barrier again.
/// Cancellation may leave the flush incomplete; repeating this is idempotent.
pub(crate) async fn prepare_existing(info: &Arc<dyn Store>) -> Result<(), ReplError> {
    if recorded::load(info).await.map_err(storage_error)?.is_some() {
        info.flush().await.map_err(storage_error)?;
    }
    Ok(())
}

pub(crate) async fn prepare_subscription(
    db: &ShamirDb,
    db_name: &str,
    repo_name: &str,
    #[cfg(test)] info_override: Option<&Arc<dyn Store>>,
) -> Result<(), ReplError> {
    #[cfg(test)]
    if let Some(info) = info_override {
        return prepare_existing(info).await;
    }
    let repo = db
        .get_db(db_name)
        .and_then(|d| d.get_repo(repo_name))
        .ok_or_else(|| ReplError::UnknownFollowerRepo {
            db: db_name.into(),
            repo: repo_name.into(),
        })?;
    let info = repo.tx_info_store().await.map_err(storage_error)?;
    prepare_existing(&info).await
}

pub(crate) fn hex_lineage(lineage: &[u8; 16]) -> String {
    lineage.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Requires `prepare_subscription` once at loop start. Any persistence error
/// ends that loop; retrying requires a fresh startup durability barrier.
/// Best-effort WARM-gate visibility enable. Presence-based and idempotent: a
/// durable v3 record for this repository enables receipt-native visibility on
/// every successful identity decision, not only at fence creation. A
/// failed/cancelled adoption flush can leave the v3 record readable without
/// being durable, so the retry over the EXISTING record must still enable.
/// Gate-init derives the flag on every later open; this only closes the gap
/// until the next restart (a zero-event follower never runs the receipt
/// pipeline, so nothing else enables it).
async fn enable_visibility(repo: &shamir_db::engine::repo::RepoInstance) {
    if let Ok(gate) = repo.tx_gate().await {
        gate.enable_receipt_native_visibility();
    }
}

pub(crate) async fn fence_hello_recorded(
    db: &ShamirDb,
    db_name: &str,
    repo_name: &str,
    response: &ReplResponse,
    receipt: Option<&SeedReceipt>,
) -> Result<(), ReplError> {
    let repo = db
        .get_db(db_name)
        .and_then(|d| d.get_repo(repo_name))
        .ok_or_else(|| ReplError::UnknownFollowerRepo {
            db: db_name.into(),
            repo: repo_name.into(),
        })?;
    let info = repo.tx_info_store().await.map_err(storage_error)?;
    hello_decide(&info, db_name, repo_name, response, receipt).await?;
    // Presence-based visibility enable (best-effort; see `enable_visibility`).
    if load_bookmark_v3(&info)
        .await
        .map_err(storage_error)?
        .is_some()
    {
        enable_visibility(&repo).await;
    }
    Ok(())
}

/// Hello decision against the same identity store used by startup and Pull.
pub(crate) async fn fence_hello_identity(
    info: &Arc<dyn Store>,
    db_name: &str,
    repo_name: &str,
    response: &ReplResponse,
    receipt: Option<&SeedReceipt>,
) -> Result<(), ReplError> {
    hello_decide(info, db_name, repo_name, response, receipt).await
}

/// Hello decision against the same identity store used by startup and Pull.
async fn hello_decide(
    info: &Arc<dyn Store>,
    db_name: &str,
    repo_name: &str,
    response: &ReplResponse,
    receipt: Option<&SeedReceipt>,
) -> Result<(), ReplError> {
    let ReplResponse::Hello { repos, .. } = response else {
        return Err(SeedRefusal::MissingIdentity.into());
    };
    let advertised = repos
        .iter()
        .find(|r| r.db == db_name && r.repo == repo_name)
        .ok_or(SeedRefusal::MissingIdentity)?;
    decide(
        info,
        advertised.repo_lineage_id,
        advertised.history_epoch,
        advertised.schema_digest.as_deref(),
        receipt,
    )
    .await
}

/// Requires the startup durability barrier; persistence failures end the loop.
/// cancel-safe: NO — a set may survive an abandoned flush; the next loop start
/// must reflush the existing record before trusting matching metadata.
/// `repo` (the destination repo instance) is used only to enable WARM-gate
/// receipt-native visibility, PRESENCE-based: any durable v3 record for this
/// repository enables it on every successful identity decision, not only at
/// creation — a failed/cancelled adoption flush can leave the record
/// readable, so the retry over the EXISTING record must still enable. Tests
/// that fence a bare probe store pass `None`.
pub(crate) async fn fence_pull_identity(
    info: &Arc<dyn Store>,
    response: &ReplResponse,
    receipt: Option<&SeedReceipt>,
    repo: Option<&shamir_db::engine::repo::RepoInstance>,
) -> Result<(), ReplError> {
    let ReplResponse::Pull {
        repo_lineage_id,
        history_epoch,
        schema_digest,
        ..
    } = response
    else {
        // Error envelopes make no identity statement and apply no events.
        return Ok(());
    };
    decide(
        info,
        *repo_lineage_id,
        *history_epoch,
        schema_digest.as_deref(),
        receipt,
    )
    .await?;
    if let Some(repo) = repo {
        if load_bookmark_v3(info)
            .await
            .map_err(storage_error)?
            .is_some()
        {
            enable_visibility(repo).await;
        }
    }
    Ok(())
}

async fn decide(
    info: &Arc<dyn Store>,
    lineage: Option<[u8; 16]>,
    epoch: Option<u64>,
    schema: Option<&str>,
    receipt: Option<&SeedReceipt>,
) -> Result<(), ReplError> {
    let existing = recorded::load(info).await.map_err(storage_error)?;
    let (Some(lineage_id), Some(history_epoch)) = (lineage, epoch) else {
        if let Some(mut record) = existing {
            record.observed_identity = None;
            recorded::save(info, record).await.map_err(storage_error)?;
        }
        return Err(SeedRefusal::MissingIdentity.into());
    };
    let observed = RepoIdentity {
        lineage_id,
        history_epoch,
    };
    let bookmark = load_replication_bookmark(info)
        .await
        .map_err(storage_error)?;
    let expected = existing
        .map(|r| RepoIdentity {
            lineage_id: r.lineage_id,
            history_epoch: r.history_epoch,
        })
        .or_else(|| receipt.map(|r| r.source_identity))
        .unwrap_or(observed);
    let mut record = existing.unwrap_or(FenceRecord {
        bookmark,
        lineage_id: expected.lineage_id,
        history_epoch: expected.history_epoch,
        observed_identity: None,
    });
    let reason = if expected.lineage_id != observed.lineage_id {
        Some(SeedRefusal::WrongIdentity)
    } else if expected.history_epoch != observed.history_epoch {
        Some(SeedRefusal::WrongHistoryEpoch)
    } else {
        receipt.and_then(|r| shamir_db::fence_metadata(r, lineage, epoch, schema).err())
    };
    // Persist observed metadata even on a refusal, without advancing the cursor.
    // Repeated matching responses need no extra write/flush.
    if existing.is_none() || record.observed_identity != Some(observed) {
        record.observed_identity = Some(observed);
        recorded::save(info, record).await.map_err(storage_error)?;
    }
    if let Some(code) = reason {
        if matches!(
            code,
            SeedRefusal::WrongIdentity | SeedRefusal::WrongHistoryEpoch
        ) {
            metrics::counter!("shamir_repl_identity_mismatch_total", "reason" => code.code())
                .increment(1);
            if receipt.is_none() {
                return Err(ReplError::IdentityMismatch {
                    code,
                    recorded: hex_lineage(&expected.lineage_id),
                    recorded_epoch: expected.history_epoch,
                    observed: hex_lineage(&observed.lineage_id),
                    observed_epoch: observed.history_epoch,
                });
            }
        }
        return Err(code.into());
    }
    if existing.is_none() && receipt.is_none() {
        tracing::warn!(bookmark, lineage = %hex_lineage(&observed.lineage_id),
            history_epoch = observed.history_epoch,
            "replication identity fence: adopted first observed leader identity (legacy migration occurs once)");
    }
    // Every successful identity decision is enough: visibility is
    // presence-based, resolved by the caller against the durable v3 record.
    Ok(())
}
