//! Group 16, Defect 1 — `create_table_context` must not silently discard a
//! `per_table_mvcc.insert_sync` collision.
//!
//! `remove_table`'s own "A13" doc comment
//! (`crates/shamir-engine/src/repo/repo_instance.rs`) already names the
//! hazard: if a stale `per_table_mvcc` entry for a table's token is left
//! behind when `create_table_context` runs again (e.g. a `remove_table(X)`
//! racing a concurrent `get_table(X)` cold-init lets a second init observe
//! the token as already-present), `scc::HashMap::insert_sync` returns `Err`
//! — and the pre-fix code discarded it with `let _ = ...`. The commit
//! pipeline resolves which `MvccStore` to write through by looking up
//! `per_table_mvcc` BY TOKEN, while the freshly-built `TableManager` this
//! call returns reads through its OWN separate (now-orphaned) store handle:
//! a split-brain where committed transactions silently vanish.
//!
//! Rather than chase the true two-`OnceCell`-chains race with real
//! concurrency (inherently timing-dependent), this test reproduces the
//! OBSERVABLE precondition directly and deterministically via the public
//! `per_table_mvcc()` accessor: pre-seed a stale entry at the table's token
//! BEFORE the table is ever attached, then attach it. `create_table_context`
//! must observe the collision the exact same way the real race would and
//! fail the table open instead of silently wiring the stale store in.

use std::sync::Arc;

use shamir_storage::error::DbError;
use shamir_storage::storage_in_memory::{InMemoryRepo, InMemoryStore};
use shamir_storage::types::Store;

use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::table_manager::table_token_for;
use crate::table::TableConfig;

fn make_instance(table: &str) -> RepoInstance {
    RepoInstance::new(
        "attach_collision".into(),
        BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
        vec![TableConfig::new(table)],
    )
}

/// Red/Green: a stale `per_table_mvcc` entry already occupying the table's
/// token MUST fail the attach outright, not succeed with the stale mapping
/// left in place.
///
/// Pre-fix: `get_table` returns `Ok(..)` (the `insert_sync` `Err` was
/// discarded), and the stale entry silently stays wired into the commit
/// pipeline forever — the exact split-brain `remove_table`'s A13 comment
/// warns about. Post-fix: `get_table` returns `Err(DbError::Internal(..))`
/// and the original stale entry is left untouched (proving no clobber
/// occurred either).
#[tokio::test]
async fn attach_fails_when_per_table_mvcc_token_already_occupied() {
    let repo = make_instance("orders");
    let token = table_token_for("orders");

    // Simulate the observable precondition of the remove_table-races-
    // get_table window: a stale MvccStore is already registered at this
    // table's token BEFORE the table is ever attached.
    let gate = repo.tx_gate().await.unwrap();
    let stale_history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let stale_mvcc = Arc::new(shamir_tx::MvccStore::new(stale_history, Arc::clone(&gate)));
    assert!(
        repo.per_table_mvcc()
            .insert_sync(token, Arc::clone(&stale_mvcc))
            .is_ok(),
        "token must be free before the pre-seed"
    );

    // The attach must now fail — not silently succeed with the stale
    // mapping still in place. (`TableManager`/`MvccStore` don't implement
    // `Debug`, so match explicitly instead of `expect_err`.)
    let err = match repo.get_table("orders").await {
        Err(e) => e,
        Ok(_) => panic!(
            "attach must fail outright on a per_table_mvcc token collision, \
             not silently succeed with a stale mapping"
        ),
    };
    assert!(
        matches!(err, DbError::Internal(_)),
        "expected DbError::Internal for the attach collision, got {err:?}"
    );

    // The stale entry must be untouched (insert_sync on collision never
    // overwrites) — the SAME Arc we pre-seeded is still the one registered.
    let still_stale = repo
        .per_table_mvcc()
        .read_sync(&token, |_, mvcc| Arc::clone(mvcc))
        .expect("the pre-seeded entry must still be present after the failed attach");
    assert!(
        Arc::ptr_eq(&still_stale, &stale_mvcc),
        "the original stale MvccStore must be left in place, not clobbered"
    );

    // A retry after the collision is cleared must succeed normally — the
    // failure is not permanently wedged.
    assert!(repo.per_table_mvcc().remove_sync(&token).is_some());
    let tbl = repo
        .get_table("orders")
        .await
        .expect("attach must succeed once the stale entry is cleared");
    assert_eq!(tbl.name(), "orders");
}
