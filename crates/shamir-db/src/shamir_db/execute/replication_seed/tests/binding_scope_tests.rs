//! S5c-X PR8b-2 evidence rows 1, 2, 7 — quarantine scope is EXACTLY the
//! subscription row's durable destination binding: a quarantined destination
//! refuses with the typed `resync_required` code while every unbound
//! destination keeps serving its actual data, and overlapping quarantines
//! clear independently.

use shamir_query_builder::batch::Batch;
#[cfg(unix)]
use shamir_query_builder::ddl::alter_subscription;
use shamir_query_builder::ddl::{repl_scope, replication_profile, subscription};
use shamir_query_builder::doc;
use shamir_query_builder::Query;
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_query_types::batch::{BatchError, BatchResponse};
use shamir_types::mpack;

use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::query::write::SetOp;
use crate::query::TableRef;
use crate::types::value::QueryValue;
use crate::ShamirDb;

/// In-memory ShamirDb with `testdb/{main,side}` and `otherdb/main`, each
/// carrying a `users` table — two databases so cross-database bindings are
/// exercised.
async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let testdb = shamir.create_db("testdb").await;
    for repo in ["main", "side"] {
        testdb
            .add_repo(
                RepoConfig::new(repo, BoxRepoFactory::in_memory())
                    .add_table(crate::engine::table::TableConfig::new("users")),
            )
            .await
            .unwrap();
    }
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

/// Insert one row into `{db}/{repo}.users` with a distinguishing `name`
/// (repo-qualified insert builder: the row must land in the repo it names).
async fn seed_row(shamir: &ShamirDb, db: &str, repo: &str, name: &str) {
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins",
        shamir_query_builder::write::Insert::with_repo(repo, "users")
            .row(doc! { "name" => name, "age" => 30 }),
    );
    shamir
        .execute(db, &b.to_request_via_msgpack())
        .await
        .unwrap();
}

/// Plain SELECT against `{db}/{repo}.users`.
async fn read_users(
    db_handle: &ShamirDb,
    db: &str,
    repo: &str,
) -> Result<BatchResponse, BatchError> {
    let mut b = Batch::new();
    b.id(9);
    b.query("r", Query::with_repo(repo, "users"));
    db_handle.execute(db, &b.to_request_via_msgpack()).await
}

/// Create a Pull profile + ACTIVE subscription whose durable binding is
/// exactly `destinations`, then quarantine the row through the REAL
/// supervisor entry point (`mark_subscription_resync_required`). The refresh
/// hook rebuilds the read-quarantine cache automatically.
async fn quarantine(shamir: &ShamirDb, sub: &str, profile: &str, destinations: &[(&str, &str)]) {
    let mut b = Batch::new();
    b.id(2);
    let mut builder = replication_profile(profile);
    for (db, repo) in destinations {
        builder = builder.stream(
            repl_scope(*db).repo(*repo).build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        );
    }
    b.op("profile", builder);
    b.op("sub", subscription(sub, "tcp://peer:9000", "pub", profile));
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    shamir
        .mark_subscription_resync_required(sub, 7, 3)
        .await
        .unwrap();
}

/// Assert the read refused with the typed quarantine code.
async fn assert_refused(shamir: &ShamirDb, db: &str, repo: &str) {
    let err = read_users(shamir, db, repo).await.unwrap_err();
    assert_eq!(
        err.code(),
        Some("resync_required"),
        "{db}/{repo} must refuse with the typed quarantine code: {err}"
    );
}

/// Assert the read SUCCEEDED and served the seeded row's actual data (parsed,
/// not just `is_ok`).
async fn assert_serves_named_row(shamir: &ShamirDb, db: &str, repo: &str, name: &str) {
    let resp = read_users(shamir, db, repo)
        .await
        .unwrap_or_else(|e| panic!("{db}/{repo} must stay readable: {e}"));
    let served = resp.results["r"].records[0].as_value().into_owned();
    assert_eq!(
        served.get("name").and_then(|v| v.as_str()),
        Some(name),
        "{db}/{repo} must serve its actual data"
    );
}

/// Persist EXACTLY what a post-reseed `Resume` persists for `name`: state
/// `active` with the durable `quarantined` marker, its receipt baseline and
/// the status message removed (the sanctioned Resume arm cannot be driven
/// here without the Unix-only seed installer).
async fn clear_subscription(shamir: &ShamirDb, name: &str) {
    let table = shamir.system_store().subscriptions_table().await.unwrap();
    let interner = table.interner_for_test().get().await.unwrap();
    let refs = crate::types::common::new_map();
    let ctx = crate::query::filter::FilterContext::new(interner, &refs);
    let mut row = table
        .read(
            &crate::query::read::ReadQuery::new("subscriptions").filter(
                crate::query::filter::Filter::Eq {
                    field: vec!["name".to_string()],
                    value: crate::query::filter::FilterValue::String(name.to_string()),
                },
            ),
            &ctx,
        )
        .await
        .unwrap()
        .records
        .remove(0)
        .as_value()
        .into_owned();
    if let QueryValue::Map(ref mut m) = row {
        m.shift_remove("quarantined");
        m.shift_remove("resync_from_receipt");
        m.shift_remove("status_message");
        m.insert("state".to_string(), QueryValue::Str("active".to_string()));
    }
    let set_op = SetOp {
        set: TableRef::new("subscriptions"),
        key: mpack!({"name": @(QueryValue::Str(name.to_string()))}),
        value: row,
    };
    shamir
        .system_store()
        .set_via_implicit_tx(&table, &set_op)
        .await
        .unwrap();
    // Rebuild the cache exactly like every admin mutation does.
    shamir.refresh_resync_quarantine_cache().await.unwrap();
}

/// Evidence row 1: quarantining `testdb/main` refuses ONLY that destination —
/// the unbound `testdb/side` and the unbound `otherdb/main` keep returning
/// their actual seeded rows.
#[tokio::test]
async fn binding_scope_single_destination_refusal_is_exact() {
    let shamir = setup().await;
    seed_row(&shamir, "testdb", "main", "Alice").await;
    seed_row(&shamir, "testdb", "side", "SideRow").await;
    seed_row(&shamir, "otherdb", "main", "OtherRow").await;
    assert!(read_users(&shamir, "testdb", "main").await.is_ok());

    quarantine(&shamir, "sub", "prof", &[("testdb", "main")]).await;

    assert_refused(&shamir, "testdb", "main").await;
    assert_serves_named_row(&shamir, "testdb", "side", "SideRow").await;
    assert_serves_named_row(&shamir, "otherdb", "main", "OtherRow").await;
}

/// Evidence row 2: ONE subscription bound to multiple destinations across
/// databases refuses EVERY bound destination; everything outside the set
/// (here `testdb/side`) stays readable with its actual data.
#[tokio::test]
async fn binding_scope_multi_destination_all_bound_refuse_others_readable() {
    let shamir = setup().await;
    seed_row(&shamir, "testdb", "main", "Alice").await;
    seed_row(&shamir, "otherdb", "main", "OtherRow").await;
    seed_row(&shamir, "testdb", "side", "SideRow").await;

    quarantine(
        &shamir,
        "sub",
        "prof",
        &[("testdb", "main"), ("otherdb", "main")],
    )
    .await;

    assert_refused(&shamir, "testdb", "main").await;
    assert_refused(&shamir, "otherdb", "main").await;
    assert_serves_named_row(&shamir, "testdb", "side", "SideRow").await;
}

/// Evidence row 7: two quarantined subscriptions OVERLAP on `testdb/main`.
/// Lifting the first quarantine (persisting exactly what a post-reseed
/// `Resume` persists) must NOT unblock anything the second still covers; only
/// when the second is lifted do the destinations serve again.
#[tokio::test]
async fn binding_scope_overlapping_quarantines_clear_independently() {
    let shamir = setup().await;
    seed_row(&shamir, "testdb", "main", "Alice").await;
    seed_row(&shamir, "testdb", "side", "SideRow").await;

    quarantine(&shamir, "sub1", "prof1", &[("testdb", "main")]).await;
    quarantine(
        &shamir,
        "sub2",
        "prof2",
        &[("testdb", "main"), ("testdb", "side")],
    )
    .await;

    // Both bound destinations refuse while both subscriptions quarantined.
    assert_refused(&shamir, "testdb", "main").await;
    assert_refused(&shamir, "testdb", "side").await;

    // Lift ONLY sub1's quarantine (persist exactly what a post-reseed
    // `Resume` persists): sub2 still covers main AND side.
    clear_subscription(&shamir, "sub1").await;
    assert_refused(&shamir, "testdb", "main").await;
    assert_refused(&shamir, "testdb", "side").await;

    // Lift sub2: its destination set is proven recovered, both serve again.
    clear_subscription(&shamir, "sub2").await;
    assert_serves_named_row(&shamir, "testdb", "main", "Alice").await;
    assert_serves_named_row(&shamir, "testdb", "side", "SideRow").await;
}

/// S5c-X round 2 (C10, Unix-only): overlap clearing through the REAL
/// recovery path — the offline installer's replacement rebind plus the real
/// admin `Resume` (probe-backed), not the hand-persisted row rewrite. Two
/// subscriptions are quarantined over `testdb/main`; rebind + Resume of
/// sub1 must NOT unblock the destination (sub2 still covers it); only after
/// sub2 is rebound and Resumed does the destination serve again.
#[cfg(unix)]
#[tokio::test]
async fn binding_scope_overlap_clearing_through_real_reseed_resume() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await;
    // No inline `users` table on `main`: the installed seed image carries
    // the table inventory (same layout as `quarantine_reads_tests::t13`).
    db.add_repo(RepoConfig::new("main", BoxRepoFactory::in_memory()))
        .await
        .unwrap();
    db.add_repo(
        RepoConfig::new("side", BoxRepoFactory::in_memory())
            .add_table(crate::engine::table::TableConfig::new("users")),
    )
    .await
    .unwrap();

    quarantine(&shamir, "sub1", "prof1", &[("testdb", "main")]).await;
    quarantine(&shamir, "sub2", "prof2", &[("testdb", "main")]).await;
    assert_refused(&shamir, "testdb", "main").await;
    assert!(read_users(&shamir, "testdb", "side").await.is_ok());

    // Real rebind of sub1 through the offline installer, then the real
    // admin `Resume`. Both rows share the SAME replacement receipt (epoch
    // 2), so one probe serves both Resumes.
    super::quarantine_reads_tests::recovery::install_replacement_seed(&shamir, "sub1", "prof1")
        .await;
    shamir.set_replication_resume_probe(super::quarantine_reads_tests::recovery::probe());
    let mut b = Batch::new();
    b.id(3);
    b.try_op("alt", alter_subscription("sub1").resume())
        .unwrap();
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // sub2's quarantine still covers the destination.
    assert_refused(&shamir, "testdb", "main").await;
    assert!(read_users(&shamir, "testdb", "side").await.is_ok());

    // Rebind + Resume sub2: the LAST lifted quarantine unblocks `main`,
    // which now serves the replacement seed's row.
    super::quarantine_reads_tests::recovery::install_replacement_seed(&shamir, "sub2", "prof2")
        .await;
    let mut b = Batch::new();
    b.id(4);
    b.try_op("alt", alter_subscription("sub2").resume())
        .unwrap();
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    let resp = read_users(&shamir, "testdb", "main").await.unwrap();
    let served = resp.results["r"].records[0].as_value().into_owned();
    assert_eq!(
        served.get("name").and_then(|v| v.as_str()),
        Some("replacement"),
        "the destination must serve the replacement seed after the full recovery"
    );
    assert!(read_users(&shamir, "testdb", "side").await.is_ok());
}
