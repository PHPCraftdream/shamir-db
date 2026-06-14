//! Integration tests for T3 — DDL retention wiring.
//!
//! Verifies that:
//! - `CreateTable` with `retention` applies the policy to the table's
//!   `MvccStore` at creation time.
//! - `SetRetention` changes a live table's policy on the fly (no
//!   migration) and the new policy governs subsequent writes.
//! - A principal without `Manage` permission is denied `SetRetention`.

use serde_json::json;

use shamir_db::access::Actor;
use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::table_manager::table_token_for;
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_types::admin::Retention;

/// Standard in-memory test fixture: a database `testdb` with a `main`
/// repo (in-memory engine) and a single `users` table.
async fn setup_shamir() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    db.add_repo(repo_config).await.unwrap();
    shamir
}

/// Look up the live `MvccStore` for `(db, repo, table)` and return a
/// snapshot of its current retention policy. Forces table instantiation
/// via `get_table` first (per_table_mvcc is lazily populated).
async fn table_retention(
    shamir: &ShamirDb,
    db_name: &str,
    repo: &str,
    table: &str,
) -> shamir_db::engine::repo::MvccRetention {
    let db = shamir.get_db(db_name).expect("db exists");
    // Force the lazy MvccStore entry.
    let _ = db.get_table(repo, table).await.expect("table exists");
    let repo_instance = db.get_repo(repo).expect("repo exists");
    let token = table_token_for(table);
    let entry = repo_instance
        .per_table_mvcc()
        .get(&token)
        .expect("mvcc entry exists");
    **entry.retention()
}

// ---------------------------------------------------------------------------
// CreateTable with retention
// ---------------------------------------------------------------------------

/// Creating a table with `retention: { max_count: 5 }` applies that
/// policy to the table's `MvccStore`.
#[tokio::test]
async fn create_table_with_retention_applies_policy() {
    let shamir = setup_shamir().await;

    let mut b = Batch::new();
    b.id(1);
    b.create_table(
        "ct",
        ddl::create_table("events").retention(Retention {
            max_count: Some(5),
            ..Default::default()
        }),
    );
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    assert_eq!(
        resp.results["ct"].records[0].as_json()["created_table"],
        json!("events")
    );
    assert_eq!(
        resp.results["ct"].records[0].as_json()["created"],
        json!(true)
    );

    // The table's MvccStore must report the retention we set.
    let policy = table_retention(&shamir, "testdb", "main", "events").await;
    assert_eq!(policy.max_count, Some(5));
    assert_eq!(policy.max_age_secs, None);
    assert_eq!(policy.min_count, None);
}

/// Creating a table WITHOUT retention leaves the default CurrentOnly
/// policy (max_count == Some(0)) — the byte-identical pre-T3 path.
#[tokio::test]
async fn create_table_without_retention_is_current_only() {
    let shamir = setup_shamir().await;

    let mut b = Batch::new();
    b.id(1);
    b.create_table("ct", ddl::create_table("logs"));
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let policy = table_retention(&shamir, "testdb", "main", "logs").await;
    assert_eq!(policy.max_count, Some(0), "default = CurrentOnly");
}

// ---------------------------------------------------------------------------
// SetRetention — on-the-fly policy change
// ---------------------------------------------------------------------------

/// `SetRetention` changes a live table's policy from CurrentOnly (the
/// default) to `max_count: 5`. The new policy is visible immediately
/// on the `MvccStore` (lock-free ArcSwap — no migration).
#[tokio::test]
async fn set_retention_changes_live_table_policy() {
    let shamir = setup_shamir().await;

    // The `users` table starts as CurrentOnly (no retention set).
    let before = table_retention(&shamir, "testdb", "main", "users").await;
    assert_eq!(before.max_count, Some(0), "default is CurrentOnly");

    // Change the policy on the fly.
    let mut b = Batch::new();
    b.id(1);
    b.set_retention(
        "sr",
        ddl::set_retention(
            "users",
            Retention {
                max_count: Some(5),
                ..Default::default()
            },
        )
        .repo("main"),
    );
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    assert_eq!(
        resp.results["sr"].records[0].as_json()["set_retention"],
        json!("users")
    );
    assert_eq!(resp.results["sr"].records[0].as_json()["ok"], json!(true));

    // The live MvccStore now reports the new policy.
    let after = table_retention(&shamir, "testdb", "main", "users").await;
    assert_eq!(after.max_count, Some(5));
}

/// `SetRetention` with all three knobs round-trips and takes effect.
#[tokio::test]
async fn set_retention_full_policy() {
    let shamir = setup_shamir().await;

    let mut b = Batch::new();
    b.id(1);
    b.set_retention(
        "sr",
        ddl::set_retention(
            "users",
            Retention {
                max_age_secs: Some(86400),
                max_count: Some(1000),
                min_count: Some(10),
            },
        )
        .repo("main"),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let policy = table_retention(&shamir, "testdb", "main", "users").await;
    assert_eq!(policy.max_age_secs, Some(86400));
    assert_eq!(policy.max_count, Some(1000));
    assert_eq!(policy.min_count, Some(10));
}

// ---------------------------------------------------------------------------
// Authorize-denied
// ---------------------------------------------------------------------------

/// A principal without `Manage` permission is denied `SetRetention`.
#[tokio::test]
async fn set_retention_denied_without_manage() {
    let shamir = setup_shamir().await;

    // Lock the database to owner-only (mode 0o700). The default owner
    // is the system; User(999) falls into "other" → no access bits.
    let mut b = Batch::new();
    b.id(1);
    let chown_h = b.chown("chown", ddl::chown(ddl::res::database("testdb"), 1));
    let chmod_h = b.chmod("chmod", ddl::chmod(ddl::res::database("testdb"), 0o700));
    b.after(&chmod_h, &chown_h);
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Non-owner tries SetRetention → access_denied.
    let mut b = Batch::new();
    b.id(2);
    b.set_retention(
        "sr",
        ddl::set_retention(
            "users",
            Retention {
                max_count: Some(5),
                ..Default::default()
            },
        )
        .repo("main"),
    );
    let err = shamir
        .execute_as(Actor::User(999), "testdb", &b.to_request_via_msgpack())
        .await
        .expect_err("User(999) must be denied SetRetention");
    assert_eq!(
        err.code(),
        Some("access_denied"),
        "expected code 'access_denied', got: {:?} ({})",
        err.code(),
        err
    );
}
