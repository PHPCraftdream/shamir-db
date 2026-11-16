//! Implicit single-op batch transactions over `&RepoCore`.
//!
//! The bodies of the repo shell's implicit-batch convenience methods live
//! here as free functions over `RepoCore`; the shell methods are one-line
//! delegates that pass `&self.core`. The
//! `Box::pin` discipline documented on `run_implicit_batch_tx` is preserved.

use crate::query::batch::BatchError;
use crate::tx::RepoCore;
use shamir_engine_query::query::write::WriteResult;
use shamir_query_types::batch::VisibilityWait;
use shamir_storage::error::DbResult;
use shamir_types::access::Actor;
use std::future::Future;
use std::pin::Pin;

/// Run a single non-tx write as an implicit single-op BATCH transaction.
///
/// F4b-1 keystone of "everything is a transaction": instead of taking the
/// direct V1 write path (which emits `begin_with_delta`/`commit` WAL
/// markers), a non-tx write opens a `IsolationLevel::Snapshot` tx on
/// this repo, stages the write via the `_tx` execute path (`stage`), and
/// commits — folding the data, index postings, and counter into ONE
/// `WalEntryV2` and consuming ONE commit version.
///
/// Snapshot isolation is deliberate: SSI validation is gated on
/// `Serializable`, so the implicit tx NEVER aborts on a read/write
/// conflict — this preserves non-tx last-writer-wins semantics.
/// Unique-index violations still surface (the unique re-validation in
/// pre-commit is unconditional) and are mapped to a coded
/// `unique_violation` error.
///
/// On a `stage` error or a commit error the `TxContext` / `SnapshotGuard`
/// drop = clean RAII abort.
///
/// `stage` receives the open `&mut TxContext` and returns the write's
/// `WriteResult`; the returned ids / shape are identical to the direct
/// path.
///
/// F5a: lifted from the private `run_implicit_batch_tx` free fn in
/// `query::batch::query_runner` so the `shamir-db` system-store and
/// admin user/role direct-delete callers can route their deletes through
/// the same implicit-tx file-WAL path (retiring the V1 DELETE marker).
///
/// F-28 Step 1 (#828): reimplemented as a thin wrapper over
/// `begin_implicit_batch_tx` / `commit_implicit_batch_tx` — the
/// `stage` HRTB closure this fn still accepts forces any captured
/// reference to satisfy `'a: 'static` (see that fn's docs), which is
/// why `query_runner`'s 4 implicit-arm call sites moved off this API onto
/// the split begin/commit pair directly. This wrapper is kept, unchanged
/// in signature and behavior, for its ~14 other existing callers.
///
/// The two delegated calls are `Box::pin`'d: without it, this debug
/// (`opt-level = 0`) build inlines `begin_implicit_batch_tx`'s and
/// `commit_implicit_batch_tx`'s full `async fn` state machines into
/// THIS future's state machine on top of the original inline body,
/// growing every caller's stack frame by the sum of both — measured to
/// tip at least one pre-existing, unrelated `#[tokio::test]` deep in a
/// DDL admin call chain (`access_ddl::group_grant_via_ddl`) over the
/// default Windows thread stack, observed as a `has overflowed its
/// stack` abort with no logic change on that path at all. Boxing moves
/// each callee's state machine to the heap, keeping this fn's own frame
/// (and thus every one of `run_implicit_batch_tx`'s ~14 callers) at
/// pre-refactor size — verified by reproducing the failure/fix with the
/// delegation form isolated in a scratch worktree before landing this.
pub(crate) async fn run_implicit_batch_tx<F>(
    repo: &RepoCore,
    actor: Actor,
    alias: &str,
    stage: F,
) -> Result<WriteResult, BatchError>
where
    F: for<'t> FnOnce(
        &'t mut shamir_tx::TxContext,
    ) -> Pin<Box<dyn Future<Output = DbResult<WriteResult>> + Send + 't>>,
{
    let (mut tx, _guard) = Box::pin(begin_implicit_batch_tx(repo, actor, alias)).await?;

    // Stage the write into the tx. On error drop tx/_guard = RAII abort.
    let wr = stage(&mut tx).await.map_err(|e| BatchError::QueryError {
        alias: alias.to_string(),
        message: e.to_string(),
        code: e.code().map(str::to_owned),
    })?;

    // Commit — folds everything into one WalEntryV2 / one commit_version.
    Box::pin(commit_implicit_batch_tx(repo, tx, alias)).await?;
    Ok(wr)
}

/// Open the implicit Snapshot batch tx exactly as `run_implicit_batch_tx`
/// does internally (set_actor + set_implicit(true)), returning the tx and
/// its SnapshotGuard so the CALLER can stage with ordinary straight-line
/// code that borrows freely (no HRTB) — see F-28 Step 1 (#828).
///
/// The returned `TxContext` / `SnapshotGuard` are plain local variables:
/// both drop cleanly (RAII abort) on any early return/error the caller
/// hits between this call and `commit_implicit_batch_tx`, exactly as
/// `run_implicit_batch_tx`'s internal `tx`/`_guard` did.
pub(crate) async fn begin_implicit_batch_tx(
    repo: &RepoCore,
    actor: Actor,
    alias: &str,
) -> Result<(shamir_tx::TxContext, shamir_tx::SnapshotGuard), BatchError> {
    begin_implicit_batch_tx_with_isolation(repo, actor, alias, shamir_tx::IsolationLevel::Snapshot)
        .await
}

/// Like `begin_implicit_batch_tx`, but lets the caller pick the isolation
/// level instead of hardcoding `Snapshot`.
///
/// F-28 Step 5 (S3-C): the query-runner's implicit DELETE/UPDATE arms use
/// this to upgrade to `Serializable` for a single implicit tx when the
/// target table is flagged (`FkReverseCache::is_fk_parent_with_delete_action`
/// / `is_fk_parent_with_update_action`) as an FK parent with a
/// non-`NoAction` `on_delete`/`on_update` — the Serializable predicate
/// check is what closes the cross-transaction TOCTOU race between the FK
/// reverse-check scan and this tx's commit
/// (see `fk_restrict.rs`/`fk_actions.rs`/`fk_on_update.rs`'s module docs).
/// Every other implicit-tx caller keeps calling
/// `begin_implicit_batch_tx`, which delegates here with `Snapshot` —
/// byte-identical behavior, unchanged.
pub(crate) async fn begin_implicit_batch_tx_with_isolation(
    repo: &RepoCore,
    actor: Actor,
    alias: &str,
    isolation: shamir_tx::IsolationLevel,
) -> Result<(shamir_tx::TxContext, shamir_tx::SnapshotGuard), BatchError> {
    let (mut tx, guard) = repo
        .begin_tx(isolation)
        .await
        .map_err(|e| BatchError::QueryError {
            alias: alias.to_string(),
            message: format!("implicit begin_tx: {}", e),
            code: None,
        })?;
    // Provenance: thread the actor for commit-time attribution (R2).
    tx.set_actor(actor);
    // Mark implicit so the changefeed event reports tx_id == 0 (preserving
    // the "0 = non-tx write" subscription contract). The internal tx_id
    // stays real for WAL / crash-injection seams.
    tx.set_implicit(true);
    Ok((tx, guard))
}

/// Commit an implicit batch tx with the canonical BatchError mapping —
/// same error-precedence/coding as `run_implicit_batch_tx`'s existing
/// commit-error handling (UniqueViolation -> "unique_violation",
/// CasConflict -> "version_conflict" per FG-7). See F-28 Step 1 (#828).
pub(crate) async fn commit_implicit_batch_tx(
    repo: &RepoCore,
    tx: shamir_tx::TxContext,
    alias: &str,
) -> Result<shamir_query_types::batch::TransactionInfo, BatchError> {
    crate::query::batch::receipts::commit_implicit(repo, tx, alias, VisibilityWait::Strict).await
}
