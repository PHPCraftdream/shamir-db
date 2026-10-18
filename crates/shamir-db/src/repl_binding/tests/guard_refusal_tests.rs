//! S5c-X round 5 — B1 (DROP DATABASE CASCADE table bug) and B4 (typed,
//! coded guard errors), through the ADMIN DISPATCH surface
//! (`shamir.execute(...)` + `err.code()`).

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{
    drop_db, drop_repo, drop_subscription, rename_repo, repl_scope, replication_profile,
    subscription,
};
use shamir_query_builder::doc;
use shamir_query_builder::Query;
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_query_types::batch::{BatchError, BatchResponse};

use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::ShamirDb;

/// In-memory ShamirDb with `testdb/main` (users table) and `otherdb/main`
/// (users table) — `otherdb` is the populated-unrelated control.
async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let testdb = shamir.create_db("testdb").await;
    testdb
        .add_repo(
            RepoConfig::new("main", BoxRepoFactory::in_memory())
                .add_table(crate::engine::table::TableConfig::new("users")),
        )
        .await
        .unwrap();
    let otherdb = shamir.create_db("otherdb").await;
    otherdb
        .add_repo(
            RepoConfig::new("main", BoxRepoFactory::in_memory())
                .add_table(crate::engine::table::TableConfig::new("users")),
        )
        .await
        .unwrap();
    shamir
}

/// Insert one row `(name, age)` into `{db}/{repo}.users`.
async fn insert_row(shamir: &ShamirDb, db: &str, repo: &str, name: &str, age: i64) {
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins",
        shamir_query_builder::write::Insert::with_repo(repo, "users")
            .row(doc! { "name" => name, "age" => age }),
    );
    shamir
        .execute(db, &b.to_request_via_msgpack())
        .await
        .unwrap();
}

/// Plain SELECT against `{db}/{repo}.users`.
async fn read_users(shamir: &ShamirDb, db: &str, repo: &str) -> Result<BatchResponse, BatchError> {
    let mut b = Batch::new();
    b.id(9);
    b.query("r", Query::with_repo(repo, "users"));
    shamir.execute(db, &b.to_request_via_msgpack()).await
}

/// Create ONE Pull profile over `testdb/main` plus an ACTIVE subscription
/// bound to it (builder-only batches).
async fn create_profile_and_subscription(shamir: &ShamirDb, sub: &str, profile: &str) {
    let mut b = Batch::new();
    b.id(2);
    b.op(
        "profile",
        replication_profile(profile).stream(
            repl_scope("testdb").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    b.op("sub", subscription(sub, "tcp://peer:9000", "pub", profile));
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
}

/// B1 (round 5): `DROP DATABASE destdb CASCADE` executed FROM the `sessdb`
/// session must drop DESTDB's tables — never the session database's
/// same-named tables (master bug: the cascade loop passed `self.db_name`).
/// Red on master's bug: the session db's `users` table would be dropped and
/// stop serving Alice while `destdb` survives.
#[tokio::test]
async fn drop_db_cascade_from_session_db_spares_session_db() {
    let shamir = setup().await;
    let sessdb = shamir.create_db("sessdb").await;
    sessdb
        .add_repo(
            RepoConfig::new("main", BoxRepoFactory::in_memory())
                .add_table(crate::engine::table::TableConfig::new("users")),
        )
        .await
        .unwrap();
    let destdb = shamir.create_db("destdb").await;
    destdb
        .add_repo(
            RepoConfig::new("main", BoxRepoFactory::in_memory())
                .add_table(crate::engine::table::TableConfig::new("users")),
        )
        .await
        .unwrap();
    insert_row(&shamir, "sessdb", "main", "Alice", 30).await;
    insert_row(&shamir, "destdb", "main", "Bob", 40).await;

    // No subscription bindings: the cascade must run — against DESTDB.
    let mut b = Batch::new();
    b.id(3);
    b.op("drop", drop_db("destdb").cascade());
    shamir
        .execute("sessdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    assert!(shamir.get_db("destdb").is_none(), "destdb must be dropped");
    assert!(
        shamir.get_db("sessdb").is_some(),
        "the session database must survive"
    );
    let resp = read_users(&shamir, "sessdb", "main").await.unwrap();
    let served = resp.results["r"].records[0].as_value().into_owned();
    assert_eq!(
        served.get("name").and_then(|v| v.as_str()),
        Some("Alice"),
        "the session db's same-named table must keep its row"
    );
}

/// B4 (round 5): DROP REPO over a bound destination is refused with the
/// typed `replication_destination_bound` code; the message names the
/// subscription AND its binding generation.
#[tokio::test]
async fn drop_repo_over_bound_destination_refused_with_typed_code() {
    let shamir = setup().await;
    insert_row(&shamir, "testdb", "main", "Alice", 30).await;
    create_profile_and_subscription(&shamir, "sub", "prof").await;

    let mut b = Batch::new();
    b.id(3);
    b.op("drop", drop_repo("main"));
    let err = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap_err();
    assert_eq!(
        err.code(),
        Some("replication_destination_bound"),
        "the bound refusal must carry the dedicated code: {err}"
    );
    assert!(
        err.to_string().contains("sub") && err.to_string().contains("binding generation"),
        "the refusal must name the subscription and its binding generation: {err}"
    );
    assert!(
        shamir.get_db("testdb").unwrap().get_repo("main").is_some(),
        "the refused drop must not remove the repo"
    );
}

/// B4 (round 5): RENAME REPO over a bound destination keeps the typed code
/// through the admin dispatch (no flattening to uncoded text).
#[tokio::test]
async fn rename_repo_over_bound_destination_preserves_typed_code() {
    let shamir = setup().await;
    insert_row(&shamir, "testdb", "main", "Alice", 30).await;
    create_profile_and_subscription(&shamir, "sub", "prof").await;

    let mut b = Batch::new();
    b.id(3);
    b.op("rename", rename_repo("main", "main2"));
    let err = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap_err();
    assert_eq!(
        err.code(),
        Some("replication_destination_bound"),
        "the rename refusal must preserve the dedicated code: {err}"
    );
    assert!(
        shamir.get_db("testdb").unwrap().get_repo("main").is_some(),
        "the refused rename must not move the repo"
    );
    assert!(read_users(&shamir, "testdb", "main").await.is_ok());
}

/// B4 (round 5): DROP SUBSCRIPTION of a quarantined subscription is refused
/// with `replication_destination_quarantined` and the truthful remedy text.
#[tokio::test]
async fn drop_quarantined_subscription_refused_with_typed_code() {
    let shamir = setup().await;
    insert_row(&shamir, "testdb", "main", "Alice", 30).await;
    create_profile_and_subscription(&shamir, "sub", "prof").await;
    shamir
        .mark_subscription_resync_required("sub", 7, 3)
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(3);
    b.op("drop", drop_subscription("sub"));
    let err = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap_err();
    assert_eq!(
        err.code(),
        Some("replication_destination_quarantined"),
        "the quarantine refusal must carry the dedicated code: {err}"
    );
    assert!(
        err.to_string().contains("in-place abandonment"),
        "the refusal must state in-place abandonment is not supported: {err}"
    );
}

/// B2 (round 5): with the terminal flush fault armed, the quarantine keys
/// are PREPUBLISHED while the row persist/flush fails — every lifecycle op
/// over the covered destination is refused with the typed codes, and a
/// populated unrelated repo stays readable.
#[tokio::test]
async fn prepublished_quarantine_cache_blocks_destination_lifecycle() {
    let shamir = setup().await;
    insert_row(&shamir, "testdb", "main", "Alice", 30).await;
    insert_row(&shamir, "otherdb", "main", "Carol", 50).await;
    create_profile_and_subscription(&shamir, "sub", "prof").await;

    // Arm the flush fault: the terminal marking prepublishes its cache keys
    // and then FAILS before durability — the keys stay until restart/refresh.
    let fault = crate::shamir_db::shamir_db::SeedCatalogueFlushFaultForTest::arm(&shamir);
    assert!(
        shamir
            .mark_subscription_resync_required("sub", 7, 3)
            .await
            .is_err(),
        "the injected flush fault must fail the terminal marking"
    );

    // DROP SUBSCRIPTION → quarantined.
    let mut b = Batch::new();
    b.id(3);
    b.op("dropsub", drop_subscription("sub"));
    let err = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap_err();
    assert_eq!(
        err.code(),
        Some("replication_destination_quarantined"),
        "{err}"
    );

    // DROP REPO → bound.
    let mut b = Batch::new();
    b.id(4);
    b.op("droprepo", drop_repo("main"));
    let err = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("replication_destination_bound"), "{err}");

    // DROP REPO CASCADE → bound.
    let mut b = Batch::new();
    b.id(5);
    b.op("dropcasc", drop_repo("main").cascade());
    let err = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("replication_destination_bound"), "{err}");

    // RENAME REPO → bound.
    let mut b = Batch::new();
    b.id(6);
    b.op("rename", rename_repo("main", "main2"));
    let err = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("replication_destination_bound"), "{err}");

    // The populated unrelated repo is untouched and readable.
    let resp = read_users(&shamir, "otherdb", "main").await.unwrap();
    let served = resp.results["r"].records[0].as_value().into_owned();
    assert_eq!(served.get("name").and_then(|v| v.as_str()), Some("Carol"));

    drop(fault);
}
