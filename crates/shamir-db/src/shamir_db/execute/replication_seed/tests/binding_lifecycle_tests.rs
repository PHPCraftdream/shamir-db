//! S5c-X PR8b-2 evidence rows 3 (in-process leg), 5, 6, 8 — the binding is
//! the durable authority through profile drops, SetProfile, profile
//! replacement, same-name recreation, terminal flush failures, and
//! DROP/RENAME of bound destinations (facade-level guards).

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{
    alter_subscription, drop_db, drop_replication_profile, drop_repo, drop_subscription,
    list_subscriptions, repl_scope, replication_profile, subscription,
};
use shamir_query_builder::doc;
use shamir_query_builder::Query;
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_query_types::batch::{BatchError, BatchResponse};
use shamir_types::mpack;

use crate::access::Actor;
use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::types::value::QueryValue;
use crate::{DbError, ShamirDb};

/// In-memory ShamirDb with `testdb/{main,side}` and `otherdb/main`, each with
/// a `users` table.
async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await;
    for repo in ["main", "side"] {
        db.add_repo(
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

/// Insert one row into `testdb/{repo}.users` (repo-qualified insert builder:
/// the row must land in the repo the helper is given).
async fn seed_row(shamir: &ShamirDb, repo: &str) {
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins",
        shamir_query_builder::write::Insert::with_repo(repo, "users")
            .row(doc! { "name" => "Alice", "age" => 30 }),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
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

/// Create ONE Pull profile over a single `(db, repo)` scope (builder-only).
async fn create_profile(shamir: &ShamirDb, profile: &str, repo: &str) {
    let mut b = Batch::new();
    b.id(2);
    b.op(
        "profile",
        replication_profile(profile).stream(
            repl_scope("testdb").repo(repo).build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
}

/// Create a profile over `testdb/{repo}` plus an ACTIVE subscription bound to
/// it, then quarantine the row through the real supervisor entry point.
async fn quarantine(shamir: &ShamirDb, sub: &str, profile: &str, repo: &str) {
    create_profile(shamir, profile, repo).await;
    let mut b = Batch::new();
    b.id(2);
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

/// Read the raw subscription row for `name` (status/list only project a
/// subset of fields — same pattern as `replication_ddl_tests`).
async fn subscription_row(shamir: &ShamirDb, name: &str) -> QueryValue {
    let table = shamir.system_store().subscriptions_table().await.unwrap();
    let interner = table.interner_for_test().get().await.unwrap();
    let refs = crate::types::common::new_map();
    let ctx = crate::query::filter::FilterContext::new(interner, &refs);
    table
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
        .into_owned()
}

/// `ALTER SUBSCRIPTION ... SET PROFILE` through the batch path.
async fn set_profile(
    shamir: &ShamirDb,
    id: u64,
    sub: &str,
    profile: &str,
) -> Result<BatchResponse, BatchError> {
    let mut b = Batch::new();
    b.id(id);
    b.try_op("alt", alter_subscription(sub).set_profile(profile))
        .unwrap();
    shamir.execute("testdb", &b.to_request_via_msgpack()).await
}

/// Evidence row 3 (in-process leg): `DROP REPLICATION PROFILE` after binding
/// leaves the bound destination refused — the refresh rebuilds coverage from
/// the row's durable binding, never from profile resolution.
#[tokio::test]
async fn binding_profile_drop_keeps_bound_destination_blocked() {
    let shamir = setup().await;
    seed_row(&shamir, "main").await;
    quarantine(&shamir, "sub", "prof", "main").await;
    assert!(read_users(&shamir, "testdb", "main").await.is_err());

    let mut b = Batch::new();
    b.id(3);
    b.op("drp", drop_replication_profile("prof"));
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let err = read_users(&shamir, "testdb", "main").await.unwrap_err();
    assert_eq!(err.code(), Some("resync_required"), "{err}");
    assert!(read_users(&shamir, "testdb", "side").await.is_ok());
}

/// Evidence row 5: SetProfile to an equal-destination profile SUCCEEDS and
/// keeps the durable binding untouched; SetProfile to a different-destination
/// profile is refused with `binding_destination_mismatch` and mutates
/// nothing; same-name subscription creation is refused with `already_exists`.
#[tokio::test]
async fn binding_set_profile_and_same_name_guards() {
    let shamir = setup().await;
    seed_row(&shamir, "main").await;
    create_profile(&shamir, "prof", "main").await;
    let mut b = Batch::new();
    b.id(2);
    b.op("sub", subscription("sub", "tcp://peer:9000", "pub", "prof"));
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Equal-destination profile: SetProfile succeeds, binding unchanged.
    create_profile(&shamir, "prof_equal", "main").await;
    set_profile(&shamir, 4, "sub", "prof_equal").await.unwrap();
    let row = subscription_row(&shamir, "sub").await;
    assert_eq!(
        row.get("profile").and_then(|v| v.as_str()),
        Some("prof_equal")
    );
    assert_eq!(row.get("binding_version").and_then(|v| v.as_u64()), Some(1));
    let destinations = row
        .get("binding_destinations")
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(destinations.len(), 1);
    assert_eq!(
        destinations[0].get("repo").and_then(|v| v.as_str()),
        Some("main")
    );
    assert!(read_users(&shamir, "testdb", "main").await.is_ok());

    // Different-destination profile: refused with the typed code; nothing
    // about the row changes.
    create_profile(&shamir, "prof_other", "side").await;
    let err = set_profile(&shamir, 6, "sub", "prof_other")
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("binding_destination_mismatch"), "{err}");
    let row = subscription_row(&shamir, "sub").await;
    assert_eq!(
        row.get("profile").and_then(|v| v.as_str()),
        Some("prof_equal"),
        "the refused SetProfile must not mutate the row"
    );

    // Same-name creation is refused with `already_exists` — a replacement
    // binding must never silently overwrite the row data was written under.
    let mut b = Batch::new();
    b.id(7);
    b.op(
        "dup",
        subscription("sub", "tcp://other:9000", "pub", "prof_equal"),
    );
    let err = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("already_exists"), "{err}");
}

/// Evidence row 5 (profile-replacement arm): recreating the REFERENCED
/// profile with a different destination set is refused with
/// `binding_destination_mismatch`; recreating it with the SAME destination
/// set succeeds; the new destination set never becomes quarantined.
#[tokio::test]
async fn binding_profile_replacement_to_different_destinations_refused_while_referenced() {
    let shamir = setup().await;
    seed_row(&shamir, "main").await;
    quarantine(&shamir, "sub", "prof", "main").await;
    assert!(read_users(&shamir, "testdb", "main").await.is_err());

    // Upsert with a DIFFERENT destination set → typed refusal.
    let mut b = Batch::new();
    b.id(3);
    b.op(
        "swap",
        replication_profile("prof").stream(
            repl_scope("testdb").repo("side").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    let err = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("binding_destination_mismatch"), "{err}");

    // The bound destination is untouched, and `side` never became special.
    let row = subscription_row(&shamir, "sub").await;
    let destinations = row
        .get("binding_destinations")
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(
        destinations[0].get("repo").and_then(|v| v.as_str()),
        Some("main")
    );
    assert!(read_users(&shamir, "testdb", "side").await.is_ok());

    // Upsert with the SAME destination set → allowed (destination-preserving
    // profile change is the one permitted mutation), and the gate holds.
    let mut b = Batch::new();
    b.id(4);
    b.op(
        "same",
        replication_profile("prof").stream(
            repl_scope("testdb").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    assert!(read_users(&shamir, "testdb", "main").await.is_err());
}

/// Evidence row 6: a terminal flush failure must not lose the prepublished
/// quarantine keys — the affected live reads stay blocked (prepublished
/// protection survived the failed persistence), unrelated destinations stay
/// readable, and after disarming the seam a successful re-mark leaves the
/// gate reflecting the persisted row.
#[tokio::test]
async fn binding_terminal_flush_failure_keeps_prepublished_protection() {
    let shamir = setup().await;
    seed_row(&shamir, "main").await;
    quarantine(&shamir, "sub", "prof", "main").await;
    assert!(read_users(&shamir, "testdb", "main").await.is_err());
    assert!(read_users(&shamir, "testdb", "side").await.is_ok());

    // A second subscription covering the previously-healthy `side`.
    create_profile(&shamir, "prof_side", "side").await;
    let mut b = Batch::new();
    b.id(2);
    b.op(
        "sub_side",
        subscription("sub_side", "tcp://peer:9000", "pub", "prof_side"),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Arm the flush fault through the RAII guard (panic-safe, self-scoping).
    let fault = crate::shamir_db::shamir_db::SeedCatalogueFlushFaultForTest::arm(&shamir);
    let err = shamir
        .mark_subscription_resync_required("sub_side", 5, 2)
        .await
        .unwrap_err();
    assert!(
        matches!(err, DbError::Storage(_)),
        "the flush seam must inject a Storage failure"
    );

    // Prepublished keys survived the failed persistence: `side` is blocked
    // even though the flush failed, and the first quarantine still holds.
    let err = read_users(&shamir, "testdb", "side").await.unwrap_err();
    assert_eq!(err.code(), Some("resync_required"), "{err}");
    assert!(read_users(&shamir, "testdb", "main").await.is_err());

    // Drop the guard (the RAII Drop disarms the fault) and re-mark:
    // persistence now completes and the refreshed cache STILL reflects the
    // persisted row (the gate never regressed below the prepublished set).
    drop(fault);
    shamir
        .mark_subscription_resync_required("sub_side", 5, 2)
        .await
        .unwrap();
    assert!(read_users(&shamir, "testdb", "side").await.is_err());
    assert!(read_users(&shamir, "testdb", "main").await.is_err());

    // An unrelated destination stays readable throughout: `otherdb/main` is
    // in NO binding, so the exact-key cache never touches it.
    assert!(read_users(&shamir, "otherdb", "main").await.is_ok());
}

/// Evidence row 8: quarantined-destination DROP/RENAME and quarantined
/// subscription DROP are refused BEFORE any live or durable mutation —
/// asserted through the FACADE APIs (`remove_db`/`rename_db_as`/
/// `remove_repo`/`rename_repo_as`), not only the admin batch path.
#[tokio::test]
async fn binding_quarantined_destination_drop_rename_and_subscription_drop_refused() {
    let shamir = setup().await;
    seed_row(&shamir, "main").await;
    quarantine(&shamir, "sub", "prof", "main").await;
    assert!(read_users(&shamir, "testdb", "main").await.is_err());

    // DROP of the quarantined subscription refused with the typed code; the
    // row is still present afterwards.
    let mut b = Batch::new();
    b.id(3);
    b.op("drop", drop_subscription("sub"));
    let err = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("resync_required"), "{err}");
    let mut b = Batch::new();
    b.id(4);
    b.op("ls", list_subscriptions());
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    let subs = resp.results["ls"].records[0]
        .as_value()
        .into_owned()
        .get("subscriptions")
        .and_then(|v| v.as_array())
        .unwrap()
        .clone();
    assert_eq!(subs.len(), 1, "the refused DROP must not delete the row");

    // Facade destination guards: every refusal names the referencing
    // subscription, and the repo survives.
    let err = shamir.remove_repo("testdb", "main").await.unwrap_err();
    assert!(err.to_string().contains("'sub'"), "{err}");
    let err = shamir
        .rename_repo_as("testdb", "main", "main2", Actor::System)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("'sub'"), "{err}");
    let err = shamir.remove_db("testdb").await.unwrap_err();
    assert!(err.to_string().contains("'sub'"), "{err}");
    let err = shamir
        .rename_db_as("testdb", "testdb2", Actor::System)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("'sub'"), "{err}");
    assert!(
        shamir.get_db("testdb").unwrap().get_repo("main").is_some(),
        "no live mutation may precede the guard refusals"
    );
    assert!(read_users(&shamir, "testdb", "main").await.is_err());

    // Negative control: an UNBOUND destination in the same db is removable —
    // the guard is exact-key, not database-wide.
    assert!(shamir.remove_repo("testdb", "side").await.unwrap());
}

/// S5c-X round 2 (C6): `DROP REPO … CASCADE` over a BOUND destination is
/// refused with the typed binding error BEFORE any cascade mutation — the
/// repo still exists and its `users` table still lists and serves its seeded
/// row afterwards. (The builder's cascade support was verified up front:
/// `shamir_query_builder::ddl::drop_repo(...).cascade()`.)
#[tokio::test]
async fn binding_cascade_drop_repo_over_bound_destination_refused_and_intact() {
    let shamir = setup().await;
    seed_row(&shamir, "main").await;
    create_profile(&shamir, "prof", "main").await;
    let mut b = Batch::new();
    b.id(2);
    b.op("sub", subscription("sub", "tcp://peer:9000", "pub", "prof"));
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(3);
    b.op("drop", drop_repo("main").cascade());
    let err = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("'sub'"),
        "the refusal must name the bound subscription: {err}"
    );

    // Nothing was dropped: the repo still lists, and the `users` table still
    // serves its actual row.
    assert!(
        shamir.get_db("testdb").unwrap().get_repo("main").is_some(),
        "the refused cascade must not remove the repo"
    );
    let resp = read_users(&shamir, "testdb", "main").await.unwrap();
    let served = resp.results["r"].records[0].as_value().into_owned();
    assert_eq!(served.get("name").and_then(|v| v.as_str()), Some("Alice"));
}

/// S5c-X round 2 (C6): `DROP DATABASE … CASCADE` over a db holding bound
/// destinations is refused with the typed binding error BEFORE the cascade —
/// every repo and table stays intact and keeps serving.
#[tokio::test]
async fn binding_cascade_drop_db_over_bound_destination_refused_and_intact() {
    let shamir = setup().await;
    seed_row(&shamir, "main").await;
    create_profile(&shamir, "prof", "main").await;
    let mut b = Batch::new();
    b.id(2);
    b.op("sub", subscription("sub", "tcp://peer:9000", "pub", "prof"));
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(3);
    b.op("drop", drop_db("testdb").cascade());
    let err = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("'sub'"),
        "the refusal must name the bound subscription: {err}"
    );

    // Every repo/table is still there and still serving its data.
    let instance = shamir.get_db("testdb").unwrap();
    assert!(instance.get_repo("main").is_some());
    assert!(instance.get_repo("side").is_some());
    let resp = read_users(&shamir, "testdb", "main").await.unwrap();
    let served = resp.results["r"].records[0].as_value().into_owned();
    assert_eq!(served.get("name").and_then(|v| v.as_str()), Some("Alice"));
    assert!(read_users(&shamir, "testdb", "side").await.is_ok());
}

/// S5c-X round 2 (C7, deterministic — no sleeps): destination lifecycle
/// shares the catalogue replication-control interlock. While the lock is
/// held, `remove_repo` has not even STARTED its first poll step (Pending);
/// once the guard drops, the removal completes.
#[tokio::test]
async fn binding_destination_lifecycle_shares_replication_control_interlock() {
    let shamir = setup().await;
    let guard = shamir.replication_control_lock().lock().await;

    let mut removal = Box::pin(shamir.remove_repo("testdb", "side"));
    assert!(
        matches!(futures::poll!(removal.as_mut()), std::task::Poll::Pending),
        "remove_repo must wait on the replication-control interlock"
    );

    drop(guard);
    assert!(
        removal.await.unwrap(),
        "the removal must succeed after unlock"
    );
}

/// S5c-X round 2 (C8): the pending-creation barrier. With the seed-catalogue
/// flush fault armed, subscription creation fails AFTER the row is durably
/// written: the raw row reads `state = pending_durable` with its full
/// binding fields, and a pending row must not gate anything. Disarming the
/// fault lets the SAME creation retry succeed (overwriting the pending row)
/// and the row reaches its final `active` state.
#[tokio::test]
async fn binding_pending_durable_barrier_survives_failed_flush_until_retry() {
    let shamir = setup().await;
    seed_row(&shamir, "main").await;
    create_profile(&shamir, "prof", "main").await;

    let fault = crate::shamir_db::shamir_db::SeedCatalogueFlushFaultForTest::arm(&shamir);
    let mut b = Batch::new();
    b.id(2);
    b.op("sub", subscription("sub", "tcp://peer:9000", "pub", "prof"));
    let err = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("seed-catalogue"),
        "the injected flush fault must surface: {err}"
    );

    // The raw row is durably PENDING: state `pending_durable`, full binding
    // fields present — but it produces no quarantine coverage.
    let row = subscription_row(&shamir, "sub").await;
    assert_eq!(
        row.get("state").and_then(|v| v.as_str()),
        Some(crate::repl_binding::pending::PENDING_STATE)
    );
    assert_eq!(row.get("binding_version").and_then(|v| v.as_u64()), Some(1));
    let binding_generation = row
        .get("binding_generation")
        .and_then(|v| v.as_u64())
        .expect("binding_generation must be present");
    assert!(
        binding_generation >= 1,
        "durable-sequence generation: monotonic, exact value depends on prior catalogue commits"
    );
    let destinations = row
        .get("binding_destinations")
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(destinations.len(), 1);
    assert_eq!(
        destinations[0].get("repo").and_then(|v| v.as_str()),
        Some("main")
    );
    assert!(
        !shamir
            .has_resync_required_subscription_covering("testdb", "main")
            .await
            .unwrap(),
        "a pending row must not gate anything"
    );

    // Disarm (drop the guard) and retry the SAME creation: overwriting a
    // `pending_durable` row is allowed, and the row reaches `active`.
    drop(fault);
    let mut b = Batch::new();
    b.id(3);
    b.op("sub", subscription("sub", "tcp://peer:9000", "pub", "prof"));
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    let row = subscription_row(&shamir, "sub").await;
    assert_eq!(row.get("state").and_then(|v| v.as_str()), Some("active"));
}

/// S5c-X round 2 (C11, corrupted-profile leg): a `replication_profiles` row
/// whose `streams` field is not decodable (a plain string) refuses any NEW
/// subscription referencing it (`seed_wrong_scope`), and corrupting the
/// profile of an EXISTING bound+quarantined subscription does NOT unblock
/// its destination — coverage is binding-based, never profile-resolution
/// based.
#[tokio::test]
async fn binding_corrupted_profile_stream_refuses_creation_and_never_unblocks() {
    let shamir = setup().await;
    seed_row(&shamir, "main").await;

    // (a) Write a corrupted profile row (streams = non-decodable Str) via
    // SetOp; creating a subscription referencing it is refused.
    let table = shamir
        .system_store()
        .replication_profiles_table()
        .await
        .unwrap();
    let mut m = crate::types::common::new_map();
    m.insert("name".to_string(), QueryValue::Str("corrupt".to_string()));
    m.insert(
        "streams".to_string(),
        QueryValue::Str("not-a-stream-list".to_string()),
    );
    let set_op = crate::query::write::SetOp {
        set: crate::query::TableRef::new("replication_profiles"),
        key: mpack!({"name": @(QueryValue::Str("corrupt".to_string()))}),
        value: QueryValue::Map(m),
    };
    shamir
        .system_store()
        .set_via_implicit_tx(&table, &set_op)
        .await
        .unwrap();
    let mut b = Batch::new();
    b.id(2);
    b.op(
        "sub",
        subscription("sub", "tcp://peer:9000", "pub", "corrupt"),
    );
    let err = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("seed_wrong_scope"), "{err}");

    // (b) An existing bound+quarantined subscription keeps its destination
    // refused even when its profile row becomes corrupted.
    create_profile(&shamir, "prof", "main").await;
    let mut b = Batch::new();
    b.id(3);
    b.op(
        "sub2",
        subscription("sub2", "tcp://peer:9000", "pub", "prof"),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    shamir
        .mark_subscription_resync_required("sub2", 7, 3)
        .await
        .unwrap();
    assert!(read_users(&shamir, "testdb", "main").await.is_err());

    // Corrupt the REFERENCED profile row the same way.
    let mut m = crate::types::common::new_map();
    m.insert("name".to_string(), QueryValue::Str("prof".to_string()));
    m.insert(
        "streams".to_string(),
        QueryValue::Str("not-a-stream-list".to_string()),
    );
    let set_op = crate::query::write::SetOp {
        set: crate::query::TableRef::new("replication_profiles"),
        key: mpack!({"name": @(QueryValue::Str("prof".to_string()))}),
        value: QueryValue::Map(m),
    };
    let table = shamir
        .system_store()
        .replication_profiles_table()
        .await
        .unwrap();
    shamir
        .system_store()
        .set_via_implicit_tx(&table, &set_op)
        .await
        .unwrap();
    let err = read_users(&shamir, "testdb", "main").await.unwrap_err();
    assert_eq!(
        err.code(),
        Some("resync_required"),
        "the corrupted profile must NOT unblock the bound destination: {err}"
    );
}

/// Read the raw subscription row for `name` if it exists (None while the
/// pending-creation barrier write has not landed yet).
async fn subscription_row_opt(shamir: &ShamirDb, name: &str) -> Option<QueryValue> {
    let table = shamir.system_store().subscriptions_table().await.ok()?;
    let interner = table.interner_for_test().get().await.ok()?;
    let refs = crate::types::common::new_map();
    let ctx = crate::query::filter::FilterContext::new(interner, &refs);
    let mut records = table
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
        .ok()?
        .records;
    if records.is_empty() {
        return None;
    }
    Some(records.remove(0).as_value().into_owned())
}

/// S5c-X round 4 (T6, deterministic — no sleeps): the cascade drop and a
/// subscription creation SERIALIZE on the replication-control interlock —
/// both batch futures are Pending while the lock is held, and the tokio
/// mutex then wakes them in poll order. Creation does NOT fail for a missing
/// destination (control.rs skips missing scopes; binding.rs only validates
/// shape), so the invariant is serialization + consistency, not refusal:
/// whichever order wins, the end state is consistent and never a partial
/// drop.
#[tokio::test]
async fn binding_cascade_interlock_serializes_create_and_never_partial_drops() {
    let shamir = setup().await;
    seed_row(&shamir, "main").await;
    let guard = shamir.replication_control_lock().lock().await;

    // Batch 1: DROP REPO main CASCADE.
    let mut b = Batch::new();
    b.id(3);
    b.op("drop", drop_repo("main").cascade());
    let request = b.to_request_via_msgpack();
    let mut cascade = Box::pin(shamir.execute("testdb", &request));

    // Batch 2: profile + ACTIVE subscription bound to testdb/main. Note
    // `op.paused = false` and no receipt on the repo, so the creation is not
    // receipt-refused.
    let mut b = Batch::new();
    b.id(4);
    b.op(
        "profile",
        replication_profile("prof").stream(
            repl_scope("testdb").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    b.op("sub", subscription("sub", "tcp://peer:9000", "pub", "prof"));
    let request = b.to_request_via_msgpack();
    let mut create = Box::pin(shamir.execute("testdb", &request));

    // BOTH wait on the control lock — serialization proven.
    assert!(
        matches!(futures::poll!(cascade.as_mut()), std::task::Poll::Pending),
        "the cascade drop must wait on the replication-control interlock"
    );
    assert!(
        matches!(futures::poll!(create.as_mut()), std::task::Poll::Pending),
        "the creation must wait on the replication-control interlock"
    );

    drop(guard);
    let cascade_result = cascade.await;
    let create_result = create.await;

    // Accept exactly the two consistent winning orders; reject everything
    // else. The end state is NEVER a partial drop.
    if cascade_result.is_ok() && create_result.is_ok() {
        // Cascade won: `testdb/main` is ABSENT, the other destinations are
        // INTACT, and the subscription row EXISTS with a binding naming
        // `testdb/main` for the absent destination (shape-only validation).
        assert!(
            shamir.get_db("testdb").unwrap().get_repo("main").is_none(),
            "the cascade must have removed testdb/main"
        );
        assert!(shamir.get_db("testdb").unwrap().get_repo("side").is_some());
        assert!(shamir.get_db("otherdb").unwrap().get_repo("main").is_some());
        assert!(read_users(&shamir, "testdb", "side").await.is_ok());
        assert!(read_users(&shamir, "otherdb", "main").await.is_ok());
        let row = subscription_row(&shamir, "sub").await;
        let destinations = row
            .get("binding_destinations")
            .and_then(|v| v.as_array())
            .unwrap();
        assert!(
            destinations
                .iter()
                .any(|d| d.get("repo").and_then(|v| v.as_str()) == Some("main")),
            "the row must bind testdb/main for the absent destination: {row:?}"
        );
    } else if cascade_result.is_err() {
        // The C6 order: the cascade was refused (the binding existed at its
        // check), so the creation ran first — ALL tables/rows are intact and
        // serve.
        create_result.expect("the creation must succeed in the C6 order");
        let instance = shamir.get_db("testdb").unwrap();
        assert!(instance.get_repo("main").is_some());
        assert!(instance.get_repo("side").is_some());
        assert!(shamir.get_db("otherdb").unwrap().get_repo("main").is_some());
        let resp = read_users(&shamir, "testdb", "main").await.unwrap();
        let served = resp.results["r"].records[0].as_value().into_owned();
        assert_eq!(served.get("name").and_then(|v| v.as_str()), Some("Alice"));
    } else {
        panic!(
            "inconsistent interlock outcome: cascade={cascade_result:?}, create={create_result:?}"
        );
    }
}

/// S5c-X round 3 (P1-C, pending barrier): with the flush fault failing the
/// FIRST flush, creation fails at the pending barrier; the row is left
/// `pending_durable`, and `ALTER SUBSCRIPTION ... RESUME` on that unfinished
/// barrier is refused with the typed `pending_creation_barrier` code while
/// the raw row stays `pending_durable`.
#[tokio::test]
async fn binding_pending_row_refuses_resume_with_typed_error() {
    let shamir = setup().await;
    seed_row(&shamir, "main").await;
    create_profile(&shamir, "prof", "main").await;

    let fault = crate::shamir_db::shamir_db::SeedCatalogueFlushFaultForTest::arm(&shamir);
    let mut b = Batch::new();
    b.id(2);
    b.op("sub", subscription("sub", "tcp://peer:9000", "pub", "prof"));
    assert!(
        shamir
            .execute("testdb", &b.to_request_via_msgpack())
            .await
            .is_err(),
        "the injected flush fault must fail the creation at the pending barrier"
    );

    // The unfinished barrier refuses EVERY admin action with the typed code.
    let mut b = Batch::new();
    b.id(3);
    b.try_op("alt", alter_subscription("sub").resume()).unwrap();
    let err = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("pending_creation_barrier"), "{err}");

    // The raw row is still the pending barrier.
    let row = subscription_row(&shamir, "sub").await;
    assert_eq!(
        row.get("state").and_then(|v| v.as_str()),
        Some(crate::repl_binding::pending::PENDING_STATE)
    );
    drop(fault);
}

/// S5c-X round 3 (P1-C, final-flip flush failure): the first flush (pending
/// barrier) succeeds, the flip flush fails, and the compensation runs — the
/// row ends `pending_durable` (durable, never durable-active). After dropping the
/// fault, the retried creation succeeds with a strictly bumped generation
/// that IS startable. Quarantine coverage stays absent throughout.
#[tokio::test]
async fn binding_final_flip_flush_failure_never_makes_row_startable() {
    let shamir = setup().await;
    seed_row(&shamir, "main").await;
    create_profile(&shamir, "prof", "main").await;

    let fault =
        crate::shamir_db::shamir_db::SeedCatalogueFlushFaultForTest::arm_after_passing(&shamir, 1);
    let mut b = Batch::new();
    b.id(2);
    b.op("sub", subscription("sub", "tcp://peer:9000", "pub", "prof"));
    assert!(
        shamir
            .execute("testdb", &b.to_request_via_msgpack())
            .await
            .is_err(),
        "the injected flip-flush fault must fail the creation batch"
    );

    // The compensated row is a durable pending barrier — never startable.
    let row = subscription_row(&shamir, "sub").await;
    assert_eq!(
        row.get("state").and_then(|v| v.as_str()),
        Some(crate::repl_binding::pending::PENDING_STATE)
    );
    let gen = row
        .get("binding_generation")
        .and_then(|v| v.as_u64())
        .expect("binding_generation must be present");
    assert!(
        !shamir
            .has_resync_required_subscription_covering("testdb", "main")
            .await
            .unwrap(),
        "a pending row must not gate anything"
    );

    // Disarm and retry: the creation completes and the row is durable-active.
    drop(fault);
    let mut b = Batch::new();
    b.id(3);
    b.op("sub", subscription("sub", "tcp://peer:9000", "pub", "prof"));
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    let row = subscription_row(&shamir, "sub").await;
    assert_eq!(row.get("state").and_then(|v| v.as_str()), Some("active"));
    let gen2 = row
        .get("binding_generation")
        .and_then(|v| v.as_u64())
        .expect("binding_generation must be present");
    assert!(gen2 > gen, "the retried creation must bump the generation");
    assert!(
        !shamir
            .has_resync_required_subscription_covering("testdb", "main")
            .await
            .unwrap(),
        "creation must not quarantine anything"
    );
}

/// S5c-X round 4 (T7, deterministic — no sleeps): the creation future is
/// DROPPED (never aborted) exactly at the final-write Notify seam — the
/// pending barrier row is durable, the final flip NEVER starts, and the row
/// can never be startable (reconcile's start gate requires `active`). The
/// catalogue stays valid throughout.
#[tokio::test]
async fn binding_cancellation_dropped_at_final_write_boundary_leaves_pending_durable() {
    let shamir = setup().await;
    seed_row(&shamir, "main").await;
    create_profile(&shamir, "prof", "main").await;

    shamir
        .final_write_pause
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let mut b = Batch::new();
    b.id(2);
    b.op("sub", subscription("sub", "tcp://peer:9000", "pub", "prof"));
    let request = b.to_request_via_msgpack();
    let mut create = Box::pin(shamir.execute("testdb", &request));
    let mut wait = Box::pin(shamir.final_write_arrived.notified());
    // Biased order polls `wait` first, so the seam's notify_one cannot be
    // missed once `wait` is registered.
    tokio::select! {
        biased;
        _ = &mut wait => {}
        res = create.as_mut() => panic!("creation ran past the final-write seam: {res:?}"),
    }
    drop(create); // DROP the parked future — never abort
    shamir
        .final_write_pause
        .store(false, std::sync::atomic::Ordering::SeqCst);

    // The dropped-at-the-boundary creation left the durable pending barrier.
    let row = subscription_row_opt(&shamir, "sub")
        .await
        .expect("the pending barrier row must be durable");
    assert_eq!(
        row.get("state").and_then(|v| v.as_str()),
        Some(crate::repl_binding::pending::PENDING_STATE),
        "the final write must never have started"
    );
    assert!(
        row.get("binding_generation")
            .and_then(|v| v.as_u64())
            .is_some(),
        "the pending barrier carries its binding generation"
    );
    // The catalogue stays valid.
    shamir.refresh_resync_quarantine_cache().await.unwrap();
    crate::repl_binding::validate::validate_subscription_catalogue(&shamir)
        .await
        .unwrap();
}
