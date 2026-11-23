//! Release of the Level-3 pessimistic record locks a parked interactive
//! transaction still holds.
//!
//! Those locks live in the per-table `MvccStore`, not in the `TxContext`, so
//! dropping the context frees none of them. The registry's `InteractiveTx`
//! therefore carries a releaser and calls it from `Drop` — the one point every
//! end of an interactive transaction passes through: the owner's rollback, the
//! idle/deadline reaper, session teardown, the `TxExecute` abort arms, and a
//! request that loses its race with any of them. Operator cancel keeps its own
//! synchronous call inside the response, through the same releaser.
//!
//! The releaser is bound to the repository RESOLVED when the handle was
//! parked, not to the database/repository NAMES: `RenameDb` / `RenameRepo`
//! re-key a live store without a gate against parked transactions, so a
//! name-resolving release would silently find nothing and leave the locks
//! held until restart.

use std::sync::Arc;

use futures_util::future::BoxFuture;
use shamir_db::engine::repo::RepoInstance;
use shamir_tx::TxContext;

use crate::tx_registry::TxAbortRelease;

/// The releaser a freshly parked transaction carries.
///
/// The transaction keeps this `RepoInstance` (a cheap clone over shared `Arc`s)
/// until the release ran, so the store survives a `RenameDb`/`RenameRepo`, or
/// a `DropDb`, that happened while the transaction was idle: the locks are then
/// released into that detached store instead of being lost, and a repository
/// dropped for good has no waiter left to wake. `None` can only mean the
/// repository vanished between `TxBegin` and the registry insert; that case is
/// logged instead of dropping the locks silently.
pub(in crate::engine::db_handler) fn tx_abort_release(
    repo: Option<RepoInstance>,
) -> TxAbortRelease {
    Arc::new(move |parked: TxContext| -> BoxFuture<'static, ()> {
        let Some(repo) = repo.clone() else {
            tracing::warn!(
                "interactive tx parked without a resolved repository: its record locks cannot be released"
            );
            return Box::pin(std::future::ready(()));
        };
        Box::pin(async move {
            shamir_db::engine::tx::release_interactive_tx_locks(&parked, repo.core()).await;
        })
    })
}
