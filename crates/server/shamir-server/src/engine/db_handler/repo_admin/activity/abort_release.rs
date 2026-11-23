//! Release of the Level-3 pessimistic record locks a parked interactive
//! transaction still holds.
//!
//! Those locks live in the per-table `MvccStore`, not in the `TxContext`, so
//! dropping the context frees none of them. The registry's `InteractiveTx`
//! therefore carries a releaser and calls it from `Drop` — the one point every
//! end of an interactive transaction passes through: the owner's rollback, the
//! idle/deadline reaper, session teardown, the `TxExecute` abort arms, and a
//! request that loses its race with any of them. Operator cancel keeps its own
//! synchronous call inside the response.

use std::sync::Arc;

use futures_util::future::BoxFuture;
use shamir_db::ShamirDb;
use shamir_tx::TxContext;

use crate::tx_registry::TxAbortRelease;

/// Resolve the transaction's pinned repository by name and release every
/// Level-3 record lock the context still holds. A database or repository that
/// no longer exists means there is nothing to release, which is not an error.
pub(in crate::engine::db_handler) async fn release_parked_locks(
    db: &ShamirDb,
    parked: &TxContext,
    db_name: &str,
    repo_name: &str,
) {
    let Some(db) = db.get_db(db_name) else {
        return;
    };
    let Some(repo) = db.get_repo(repo_name) else {
        return;
    };
    shamir_db::engine::tx::release_interactive_tx_locks(parked, repo.core()).await;
}

/// The releaser every parked interactive transaction carries.
/// `InteractiveTx::drop` hands it the leftover context when the last reference
/// is gone, and `shamir-sync` spawns the returned future because `Drop` cannot
/// await.
pub(in crate::engine::db_handler) fn tx_abort_release(db: Arc<ShamirDb>) -> TxAbortRelease {
    Arc::new(
        move |parked: TxContext, db_name: String, repo_name: String| -> BoxFuture<'static, ()> {
            let db = Arc::clone(&db);
            Box::pin(async move {
                release_parked_locks(&db, &parked, &db_name, &repo_name).await;
            })
        },
    )
}
