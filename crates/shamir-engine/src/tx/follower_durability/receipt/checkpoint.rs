//! Step 4: read back / validate journal coverage, then atomically
//! checkpoint bookmark+J and retire the receipt.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{AtomicBatch, AtomicCommitOutcome, RecordKey, Store};
use shamir_tx::version_key;

use super::stage;
use crate::meta::repl::bookmark_v3::{encode_bookmark_v3, load_bookmark_v3, BookmarkV3};
use crate::meta::repl::receipt::receipt_key;
use crate::meta::repl::ReplicationReceipt;
use crate::repo::RepoInstance;

const TX_STORE: &str = "__tx__";

/// Matches `barrier::CONFIRM_BUDGET` (private to the barrier module).
const RECEIPT_CONFIRM_BUDGET: Duration = Duration::from_secs(1);

/// Every receipt event must already be in the follower journal, byte-exact.
/// The journal flush afterwards is THE durability boundary for the journal
/// rows — it happens BEFORE any bookmark advance.
pub(crate) async fn verify_journal(
    repo: &RepoInstance,
    receipt: &ReplicationReceipt,
) -> DbResult<()> {
    let store = repo.follower_journal_store().await?;
    for (index, mapping) in receipt.mapping.iter().enumerate() {
        let expected = receipt.downstream_events.get(index).ok_or_else(|| {
            DbError::RepoRequiresReopen(
                "receipt downstream events are shorter than its mapping".into(),
            )
        })?;
        match store
            .get(RecordKey::from_slice(&version_key(mapping.local_version)))
            .await
        {
            Ok(actual) if actual.as_ref() == expected.as_slice() => {}
            Ok(_) | Err(DbError::NotFound(_)) => {
                return Err(DbError::Storage(format!(
                    "follower receipt journal coverage missing/mismatched at local version {}",
                    mapping.local_version
                )));
            }
            Err(error) => return Err(error),
        }
    }
    store.flush().await
}

/// Confirm the journal feed through `local_end` so the clean horizon J may
/// advance over the batch.
pub(crate) async fn confirm_journal(repo: &RepoInstance, local_end: u64) -> DbResult<()> {
    let handle = repo.changefeed().await?;
    let confirmed = handle
        .feed
        .confirm_through(local_end, RECEIPT_CONFIRM_BUDGET)
        .await;
    if confirmed < local_end {
        return Err(DbError::Storage(format!(
            "follower receipt journal confirmation stopped at {confirmed}, required {local_end}"
        )));
    }
    Ok(())
}

fn local_end(receipt: &ReplicationReceipt) -> u64 {
    receipt
        .mapping
        .iter()
        .map(|mapping| mapping.local_version)
        .max()
        .unwrap_or(0)
}

async fn stage_checkpoint_batch(
    repo: &RepoInstance,
    receipt: &ReplicationReceipt,
    old: &BookmarkV3,
) -> DbResult<Box<dyn AtomicBatch>> {
    let end = local_end(receipt);
    let bookmark = BookmarkV3 {
        fence: old.fence.clone(),
        bookmark: old.bookmark.max(receipt.upstream_end),
        clean_through: Some(old.clean_through.map_or(end, |j| j.max(end))),
        outstanding_receipt: None,
    };
    let mut batch = repo.begin_repo_atomic_batch().await?;
    batch.set(
        TX_STORE,
        stage::bookmark_record_key(),
        Bytes::from(encode_bookmark_v3(&bookmark)?),
    )?;
    // The receipt retires ATOMICALLY with the bookmark advance: the
    // `Checkpointed` phase is therefore never durably observable — a crash
    // leaves either Prepared/Materialized + old bookmark, or retired receipt
    // + advanced bookmark. That is intentional.
    batch.delete(TX_STORE, receipt_key())?;
    Ok(batch)
}

/// Step 4b: advance the bookmark, J, and retire the receipt in ONE atomic
/// batch; flush the info store afterwards.
pub(crate) async fn checkpoint(
    repo: &RepoInstance,
    info: &Arc<dyn Store>,
    receipt: &ReplicationReceipt,
    old: &BookmarkV3,
) -> DbResult<()> {
    let outcome = stage_checkpoint_batch(repo, receipt, old)
        .await?
        .commit()
        .await?;
    if let AtomicCommitOutcome::Uncertain { detail } = outcome {
        // Reconcile by reading back: advanced bookmark AND retired receipt.
        let settled = match load_bookmark_v3(info).await {
            Ok(Some(bookmark)) => {
                bookmark.bookmark >= receipt.upstream_end && bookmark.outstanding_receipt.is_none()
            }
            _ => false,
        };
        if !settled {
            match stage_checkpoint_batch(repo, receipt, old)
                .await?
                .commit()
                .await?
            {
                AtomicCommitOutcome::Committed => {}
                AtomicCommitOutcome::Uncertain { detail } => {
                    return Err(DbError::RepoRequiresReopen(format!(
                        "receipt checkpoint commit uncertain twice: {detail}"
                    )));
                }
            }
        } else {
            log::warn!(
                "receipt checkpoint commit was uncertain but the read-back shows \
                 the bookmark advanced (repo {}): {detail}",
                repo.name()
            );
        }
    }
    #[cfg(test)]
    super::super::test_seam::phase(
        repo.name(),
        super::super::test_seam::Phase::ReceiptCheckpointed,
    )
    .await;
    log::info!(
        "follower receipt: Checkpointed upstream {}..={} on repo {}",
        receipt.upstream_start,
        receipt.upstream_end,
        repo.name()
    );
    info.flush().await
}
