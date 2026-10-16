//! Cold-start completion of an outstanding replication receipt (S5c-X
//! PR6b-2). Runs inside `reconcile_with_hook` BEFORE the PR6c classification
//! so a completed receipt's versions are certified through the bookmark's
//! clean horizon instead of being declared a recovery GAP.
//!
//! Idempotent: a no-op when the bookmark v3 record is absent or references
//! no outstanding receipt. Otherwise it drives the SAME atomic
//! Prepared → Materialized → checkpoint pipeline the warm resume path uses
//! (`receipt::stage` / `receipt::checkpoint`, including the Uncertain
//! read-back reconciles), minus guard publication — a cold start has no live
//! gate watermark obligation; the cold gate seeds from the persisted markers.
//! The checkpoint tail also runs the synchronous RAM publication
//! (`receipt::publish`) exactly like the warm path. Journal coverage is
//! verified (and repaired through the sole journal writer if an Uncertain
//! commit lost rows) before any bookmark advance, so any decode/IO failure
//! refuses the cold open, matching the PR4 policy.

use std::sync::Arc;

use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::Store;

use crate::meta::repl::bookmark_v3::{load_bookmark_v3, BookmarkV3};
use crate::meta::repl::receipt::load_outstanding_receipt;
use crate::meta::repl::{ReceiptPhase, ReplicationReceipt};
use crate::repo::RepoInstance;
use crate::tx::apply_replicated::follower_durability::receipt::{checkpoint, publish, stage};

/// Complete the receipt referenced by the persisted bookmark v3 pointer, if
/// any. Must run before the cold classification reads the clean horizon.
///
/// Admission (PR6b-2c): the pipeline stages admit through the repository
/// write-domain permit; a cold start is uncontended at open, so a plain
/// `admit()` cannot fail in practice — any rejection is fatal to the open.
pub(crate) async fn complete_outstanding_receipt(
    repo: &RepoInstance,
    info: &Arc<dyn Store>,
) -> DbResult<()> {
    let Some(v3) = load_bookmark_v3(info).await? else {
        return Ok(());
    };
    let Some(id) = v3.outstanding_receipt else {
        return Ok(());
    };
    let Some(receipt) = load_outstanding_receipt(info, &id).await? else {
        return Err(DbError::RepoRequiresReopen(
            "bookmark references a missing receipt (cold start)".into(),
        ));
    };
    if receipt.phase == ReceiptPhase::Checkpointed {
        return Err(DbError::RepoRequiresReopen(
            "bookmark references a Checkpointed receipt; unreachable state".into(),
        ));
    }
    let permit = repo.write_domain().admit().await.map_err(|error| {
        DbError::Internal(format!(
            "cold receipt recovery admission rejected for repo {}: {error}",
            repo.name()
        ))
    })?;

    let old = BookmarkV3 {
        fence: v3.fence,
        bookmark: receipt.predecessor_bookmark,
        clean_through: v3.clean_through,
        outstanding_receipt: None,
    };
    let work = match receipt.phase {
        ReceiptPhase::Prepared => stage::materialize_from_prepared_receipt(repo, &receipt).await?,
        ReceiptPhase::Materialized => stage::materialize_from_materialized_receipt(&receipt)?,
        ReceiptPhase::Checkpointed => unreachable!("checked above"),
    };
    let (receipt, _counters) =
        stage::commit_materialized_work(repo, &permit, info, &work, &old).await?;

    // Durability boundary: verify journal coverage (the Materialized batch
    // wrote the rows; only an Uncertain commit that silently failed can leave
    // them missing) and repair through the WRITER, never direct writes.
    if let Err(first) = checkpoint::verify_journal(repo, &receipt).await {
        log::warn!(
            "cold receipt recovery: journal coverage incomplete ({first}); \
             repairing via install_durable_events"
        );
        let handle = repo.changefeed().await?;
        let records = stage::journal_records(&receipt)?;
        handle
            .feed
            .install_durable_events(records)
            .await
            .map_err(|error| DbError::Storage(format!("cold receipt journal repair: {error}")))?;
        checkpoint::verify_journal(repo, &receipt).await?;
    }
    let local_end = local_end(&receipt);
    checkpoint::confirm_journal(repo, local_end).await?;
    let publication = publish::publication_from_receipt(repo, &receipt).await?;
    publish::adopt_counters(repo, &publication).await?;
    let gate = repo.tx_gate().await?;
    // One atomic batch: bookmark advance + receipt retirement, with the RAM
    // publication in the same poll (+ info flush).
    checkpoint::checkpoint(repo, &permit, info, &receipt, &old, &gate, &publication).await?;
    log::info!(
        "cold receipt recovery: completed receipt upstream {}..={} through \
         local version {} on repo {}",
        receipt.upstream_start,
        receipt.upstream_end,
        local_end,
        repo.name()
    );
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
