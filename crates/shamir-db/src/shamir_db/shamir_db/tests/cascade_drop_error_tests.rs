//! P0 cascade-drop regression test (RV-DB-42E3530E).
//!
//! `DROP REPO CASCADE` / `DROP DATABASE CASCADE` must COLLECT per-table
//! drop errors and fail with the typed `cascade_drop_failed` code instead
//! of swallowing them (`let _ =`) and reporting success while catalogue
//! rows (and possibly whole repos) were left inconsistent.
//!
//! Failure injection: the first `SystemStore` write on the per-table drop
//! path is `remove_table` (`drop_table_checked` performs no catalogue
//! writes; with no validators bound, `persist_validator_bound_in` is
//! skipped and `drop_schema_validator` is in-memory only). One armed fault
//! therefore lands on the first table's `remove_table`.
//!
//! Pre-fix the per-table error was swallowed and the DROP returned Ok, so
//! the `is_err()` assertion below fails on pre-fix code.

use shamir_engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_engine::VisibleOutcome;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;

use crate::shamir_db::ShamirDb;

fn assert_cascade_drop_failed(err: crate::query::batch::BatchError, what: &str) {
    match err {
        crate::query::batch::BatchError::QueryError { code, message, .. } => {
            assert_eq!(
                code.as_deref(),
                Some("cascade_drop_failed"),
                "{what}: expected cascade_drop_failed code, got code={code:?}, msg={message}"
            );
            assert!(
                message.contains("cascade table drops failed"),
                "{what}: message must name the cascade table-drop failures, got: {message}"
            );
        }
        other => panic!("{what}: expected QueryError, got {other:?}"),
    }
}

#[tokio::test]
async fn rv_db_42e3530e_drop_repo_cascade_reports_table_drop_failure() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("t").await.expect("create_db");

    let mut batch = Batch::new();
    batch.id(1);
    batch.create_repo("main", ddl::create_repo("main").tables(["alpha", "beta"]));
    shamir
        .execute("t", &batch.to_request_via_msgpack())
        .await
        .expect("create_repo with two tables must succeed");

    // One fault: consumed by the first table's `remove_table` in the
    // cascade loop.
    shamir.system_store().arm_catalogue_write_fault(1);

    let mut drop = Batch::new();
    drop.id(2);
    drop.drop_repo("dr", ddl::drop_repo("main").cascade());
    let err = shamir
        .execute("t", &drop.to_request_via_msgpack())
        .await
        .expect_err("RV-DB-42E3530E: DROP REPO CASCADE must fail when a table drop fails");
    assert_cascade_drop_failed(err, "drop repo cascade");

    // The repo must NOT have been removed — the cascade aborted.
    assert!(
        shamir.get_db("t").expect("db 't' exists").has_repo("main"),
        "the repository must survive a failed cascade drop"
    );
}

#[tokio::test]
async fn rv_db_42e3530e_drop_db_cascade_reports_table_drop_failure() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("u").await.expect("create_db");

    let mut batch = Batch::new();
    batch.id(1);
    batch.create_repo("main", ddl::create_repo("main").tables(["alpha", "beta"]));
    shamir
        .execute("u", &batch.to_request_via_msgpack())
        .await
        .expect("create_repo with two tables must succeed");

    shamir.system_store().arm_catalogue_write_fault(1);

    let mut drop = Batch::new();
    drop.id(2);
    drop.drop_db("dd", ddl::drop_db("u").cascade());
    let err = shamir
        .execute("u", &drop.to_request_via_msgpack())
        .await
        .expect_err("RV-DB-42E3530E: DROP DATABASE CASCADE must fail when a table drop fails");
    assert_cascade_drop_failed(err, "drop db cascade");

    // The database must NOT have been removed — the cascade aborted.
    assert!(
        shamir.has_db("u"),
        "the database must survive a failed cascade drop"
    );
}

// ============================================================================
// bounded changelog journal wrappers — merged from
// bounded_journal_read_tests.rs (layout slot consolidation).
// ============================================================================

// Baseline exposes only Option<JournalRead>, collapsing errors and absence,
// and has no completion-progress facade for an idle/empty repository.
#[tokio::test]
async fn bounded_journal_wrappers_distinguish_missing_and_present_repo() {
    let db = ShamirDb::init_memory().await.unwrap();
    assert!(db
        .read_changelog_bounded("missing-db", "main", 1, 10)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        db.await_changelog_progress("missing-db", "main", 1)
            .await
            .unwrap(),
        None,
    );
    db.create_db("journal-db").await.expect("create_db");
    assert!(db
        .read_changelog_bounded("journal-db", "missing-repo", 1, 10)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        db.await_changelog_progress("journal-db", "missing-repo", 1)
            .await
            .unwrap(),
        None,
    );
    db.add_repo(
        "journal-db",
        RepoConfig::new("main", BoxRepoFactory::in_memory()),
    )
    .await
    .unwrap();
    let page = db
        .read_changelog_bounded("journal-db", "main", 1, 10)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((page.sampled_w, page.bound, page.confirmed), (0, 0, true));
    assert!(page.read.events.is_empty());
    assert_eq!(page.read.gap_at, None);
    assert_eq!(
        db.await_changelog_progress("journal-db", "main", 0)
            .await
            .unwrap(),
        Some(VisibleOutcome::Visible),
    );
    // `Ok(None)` means only that the db/repo is absent; real journal errors
    // propagate as `Err` instead of being swallowed into `Ok(None)`.
    let legacy = db
        .read_changelog_from_journal("journal-db", "main", 1, 10)
        .await
        .unwrap()
        .expect("repo exists");
    assert_eq!(legacy.events, page.read.events);
    assert_eq!(legacy.gap_at, page.read.gap_at);
}
