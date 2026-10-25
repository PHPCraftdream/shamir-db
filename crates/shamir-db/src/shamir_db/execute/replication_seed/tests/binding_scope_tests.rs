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

use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
#[cfg(unix)]
use crate::shamir_db::SystemStoreConfig;
use crate::types::value::QueryValue;
use crate::ShamirDb;

#[cfg(unix)]
/// Re-open a durable ShamirDb on the same system-store path, retrying
/// briefly while the dropped instance's store still holds the fjall file lock
/// (the lock is released lazily after the drop).
async fn reopen_durable(sys_path: std::path::PathBuf) -> ShamirDb {
    for _ in 0..100 {
        match ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone())).await {
            Ok(db) => return db,
            Err(e) => {
                let message = e.to_string();
                assert!(
                    message.contains("Locked"),
                    "unexpected reopen error: {message}",
                );
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }
    }
    panic!("fjall lock was not released within the retry window");
}

/// In-memory ShamirDb with `testdb/{main,side}` and `otherdb/main`, each
/// carrying a `users` table — two databases so cross-database bindings are
/// exercised.
async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let testdb = shamir.create_db("testdb").await.expect("create_db");
    for repo in ["main", "side"] {
        testdb
            .add_repo(
                RepoConfig::new(repo, BoxRepoFactory::in_memory())
                    .add_table(crate::engine::table::TableConfig::new("users")),
            )
            .await
            .unwrap();
    }
    let otherdb = shamir.create_db("otherdb").await.expect("create_db");
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
/// here without the Unix-only seed installer). Round 6 item 1: persist
/// through the SAME true-replace path as the production Resume arm — a
/// plain `SetOp` MERGES, so the `shift_remove`ed fields would survive
/// durably and the marker would never leave the row.
/// COUNTERFACTUAL: reverting this helper to `set_via_implicit_tx` turns the
/// durable-row assertions in `binding_scope_overlapping_quarantines_clear_
/// independently` and the resume tests red (`quarantined: true` survives).
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
    shamir
        .replace_subscription_row(&table, name.to_string(), row)
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

/// S5c-X round 7 item 2 (C10, Unix-only): COVERAGE-AGGREGATION through the
/// REAL recovery path. Fixture admission rule: `matches_scope`
/// (`catalogue.rs`) requires a subscription's scope to be EXACTLY ONE
/// stream, so each row here keeps a SINGLE-stream scope (`testdb/main`)
/// and the overlap is shown by TWO single-destination subscriptions bound
/// to the SAME destination. Resuming sub1 must NOT unblock `testdb/main`:
/// sub2's quarantined row still covers it. Two quarantined subscriptions
/// overlapping one destination are NOT simultaneous active writers — the
/// protection is aggregation over every covering row.
/// COUNTERFACTUAL: reverting to pre-round-6 semantics (a resumed row clears
/// the shared destination's protection, or `check_owners` treats the
/// quarantined sibling as a competing writer) turns the still-refused
/// `testdb/main` assertion red. The test ENDS after proving independence:
/// resolving sub2 would require a second replacement reseed, and the
/// aggregation rule does not need a second active writer to be shown.
#[cfg(unix)]
#[tokio::test]
async fn binding_scope_overlap_clearing_through_real_reseed_resume() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await.expect("create_db");
    // No inline `users` table on `main`: the installed seed image carries
    // the table inventory (same layout as `quarantine_reads_tests::t13`).
    db.add_repo(RepoConfig::new("main", BoxRepoFactory::in_memory()))
        .await
        .unwrap();

    // BOTH rows are single-stream (`matches_scope` admission rule) and
    // overlap on the ONE destination `testdb/main`.
    quarantine(&shamir, "sub1", "prof1", &[("testdb", "main")]).await;
    quarantine(&shamir, "sub2", "prof2", &[("testdb", "main")]).await;
    assert_refused(&shamir, "testdb", "main").await;

    // Real recovery: install the replacement image ONCE, then rebind BOTH
    // rows — each keeps its SINGLE-stream `testdb/main` scope. They share
    // the SAME replacement receipt, so the round-3 same-receipt
    // `check_owners` rule keeps the quarantined siblings from competing
    // during each Resume.
    super::quarantine_reads_tests::recovery::install_replacement_image(&shamir).await;
    super::quarantine_reads_tests::recovery::rebind_to_replacement_covering(
        &shamir,
        "sub1",
        "prof1",
        &[("testdb", "main")],
    )
    .await;
    super::quarantine_reads_tests::recovery::rebind_to_replacement_covering(
        &shamir,
        "sub2",
        "prof2",
        &[("testdb", "main")],
    )
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

    // The SHARED destination is STILL covered by sub2's quarantined row —
    // coverage aggregation, not a competing active writer. (sub1 has no
    // exclusive destination: its single bound destination is the shared
    // one, and its durable marker removal is covered by
    // `binding_scope_resume_removes_quarantine_marker_durably`.)
    assert_refused(&shamir, "testdb", "main").await;
}

/// Round 6 item 1: after quarantine → REAL replacement install + rebind →
/// admin `Resume`, the DURABLE row must have `quarantined`, `status_message`
/// and `resync_from_receipt` REMOVED and `state == "active"` (Resume
/// persists through `replace_subscription_row`: ONE implicit tx staging the
/// WHOLE replacement bytes).
/// COUNTERFACTUAL: restoring the merge-SET persist (`set_via_implicit_tx`)
/// keeps `quarantined: true` on the durable row (the `execute_set_tx` merge
/// branch copies omitted keys' old bytes verbatim) and turns the three
/// field-absent assertions red.
#[cfg(unix)]
#[tokio::test]
async fn binding_scope_resume_removes_quarantine_marker_durably() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await.expect("create_db");
    db.add_repo(RepoConfig::new("main", BoxRepoFactory::in_memory()))
        .await
        .unwrap();
    quarantine(&shamir, "sub", "prof", &[("testdb", "main")]).await;
    assert_refused(&shamir, "testdb", "main").await;

    super::quarantine_reads_tests::recovery::install_replacement_image(&shamir).await;
    super::quarantine_reads_tests::recovery::rebind_to_replacement(&shamir, "sub", "prof").await;
    shamir.set_replication_resume_probe(super::quarantine_reads_tests::recovery::probe());
    let mut b = Batch::new();
    b.id(3);
    b.try_op("alt", alter_subscription("sub").resume()).unwrap();
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Read the DURABLE row: the three marker fields must be GONE (not
    // merely stale in some cache).
    let records = super::super::catalogue::rows(&shamir, "subscriptions")
        .await
        .unwrap();
    let row = records
        .iter()
        .find(|r| r.get("name").and_then(|v| v.as_str()) == Some("sub"))
        .expect("subscription row must exist");
    assert_eq!(row.get("state").and_then(|v| v.as_str()), Some("active"));
    assert_eq!(
        row.get("quarantined"),
        None,
        "the durable `quarantined` marker must be REMOVED, not merged back"
    );
    assert_eq!(row.get("status_message"), None);
    assert_eq!(row.get("resync_from_receipt"), None);
}

/// Round 6 item 1 (REOPEN leg): the marker removal survives a full reopen —
/// after Resume + restart the durable row still has NO `quarantined` field
/// and the destination serves the replacement seed.
/// COUNTERFACTUAL: a merge-SET Resume writes `quarantined: true` back into
/// the reopened row (the cache is rebuilt from persisted rows at init), so
/// the destination would refuse again after the restart.
#[cfg(unix)]
#[tokio::test]
async fn binding_scope_resume_marker_removal_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta");
    let repo_path = dir.path().join("main");

    {
        let shamir = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
            .await
            .unwrap();
        shamir.create_db("testdb").await.expect("create_db");
        shamir
            .add_repo(
                "testdb",
                // RAW fjall (no MemBuffer): this test binds a SUBSCRIPTION to
                // this repo, and the CREATE/RESUME admission
                // (validate_repo_wide_atomic_batch) refuses MemBuffer-wrapped
                // destinations with `atomic_batch_unsupported`. Raw fjall is also
                // the backend the seed writes through, so the durable directory
                // survives the reopen below.
                RepoConfig::new("main", BoxRepoFactory::fjall_raw(repo_path.clone())),
            )
            .await
            .unwrap();
        quarantine(&shamir, "sub", "prof", &[("testdb", "main")]).await;
        assert_refused(&shamir, "testdb", "main").await;

        super::quarantine_reads_tests::recovery::install_replacement_image(&shamir).await;
        super::quarantine_reads_tests::recovery::rebind_to_replacement(&shamir, "sub", "prof")
            .await;
        shamir.set_replication_resume_probe(super::quarantine_reads_tests::recovery::probe());
        let mut b = Batch::new();
        b.id(3);
        b.try_op("alt", alter_subscription("sub").resume()).unwrap();
        shamir
            .execute("testdb", &b.to_request_via_msgpack())
            .await
            .unwrap();
    }

    // Reopen with the bounded fjall-lock retry (mirrors `durable_repo_tests`).
    let mut shamir = None;
    for _ in 0..100 {
        match ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone())).await {
            Ok(opened) => {
                shamir = Some(opened);
                break;
            }
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
    let shamir = shamir.expect("system store still locked after retries");

    // The marker STAYS gone after the reopen and the destination serves.
    let records = super::super::catalogue::rows(&shamir, "subscriptions")
        .await
        .unwrap();
    let row = records
        .iter()
        .find(|r| r.get("name").and_then(|v| v.as_str()) == Some("sub"))
        .expect("subscription row must exist");
    assert_eq!(row.get("quarantined"), None);
    // Round 7 item 1: ALL THREE marker fields must stay gone after the
    // reopen (not only `quarantined`).
    assert_eq!(row.get("status_message"), None);
    assert_eq!(row.get("resync_from_receipt"), None);
    assert_eq!(row.get("state").and_then(|v| v.as_str()), Some("active"));
    let resp = read_users(&shamir, "testdb", "main").await.unwrap();
    let served = resp.results["r"].records[0].as_value().into_owned();
    assert_eq!(
        served.get("name").and_then(|v| v.as_str()),
        Some("replacement")
    );
}

/// Round 7 item 1: DIRECT test on the true-replace helper. An existing row
/// is replaced with a modified record — one field CHANGED (`upstream`) and
/// the marker fields REMOVED (exactly what the Resume arm persists) — and a
/// fresh read proves the removal reached durability and the change took
/// (no merge with the old bytes: the helper stages
/// `KvOp::Set(id, value.to_bytes())` through `update_tx`, the WHOLE new
/// record).
/// COUNTERFACTUAL: persisting through a merge-SET (`set_via_implicit_tx`)
/// copies omitted keys' old bytes verbatim, so `quarantined`,
/// `status_message` and `resync_from_receipt` reappear on the fresh read.
#[tokio::test]
async fn replace_subscription_row_true_replaces_fields() {
    let shamir = setup().await;
    quarantine(&shamir, "sub", "prof", &[("testdb", "main")]).await;

    let table = shamir.system_store().subscriptions_table().await.unwrap();
    let rows = super::super::catalogue::rows(&shamir, "subscriptions")
        .await
        .unwrap();
    let mut record = rows
        .iter()
        .find(|r| r.get("name").and_then(|v| v.as_str()) == Some("sub"))
        .expect("subscription row must exist")
        .clone();
    // The durable row carries the markers after the mark.
    assert_eq!(
        record.get("quarantined").and_then(|v| v.as_bool()),
        Some(true)
    );
    if let QueryValue::Map(ref mut m) = record {
        m.shift_remove("quarantined");
        m.shift_remove("status_message");
        m.shift_remove("resync_from_receipt");
        m.shift_remove("last_terminal_reason");
        m.insert("state".to_string(), QueryValue::Str("active".to_string()));
        // A CHANGED field: must carry the new value, not the merged old one.
        m.insert(
            "upstream".to_string(),
            QueryValue::Str("tcp://peer:9100".to_string()),
        );
    }
    shamir
        .replace_subscription_row(&table, "sub".to_string(), record)
        .await
        .unwrap();

    // FRESH read from the store (no cache): removals + change are durable.
    let rows = super::super::catalogue::rows(&shamir, "subscriptions")
        .await
        .unwrap();
    let row = rows
        .iter()
        .find(|r| r.get("name").and_then(|v| v.as_str()) == Some("sub"))
        .expect("replaced row must exist");
    assert_eq!(row.get("quarantined"), None);
    assert_eq!(row.get("status_message"), None);
    assert_eq!(row.get("resync_from_receipt"), None);
    assert_eq!(row.get("last_terminal_reason"), None);
    assert_eq!(row.get("state").and_then(|v| v.as_str()), Some("active"));
    assert_eq!(
        row.get("upstream").and_then(|v| v.as_str()),
        Some("tcp://peer:9100"),
        "the whole-record replace must persist the CHANGED field"
    );
}

/// COUNTERFACTUAL: generation-only claim lookup leaks A after G2; prefix
/// matching consumes B too. The final serve/refuse checks distinguish both.
#[cfg(unix)]
#[tokio::test]
async fn resume_releases_stale_same_name_claim_after_recovery() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await.expect("create_db");
    db.add_repo(RepoConfig::new("main", BoxRepoFactory::in_memory()))
        .await
        .unwrap();
    quarantine(&shamir, "sub", "prof", &[("testdb", "main")]).await;
    let fault = crate::shamir_db::shamir_db::SeedCatalogueFlushFaultForTest::arm(&shamir);
    assert!(shamir
        .mark_subscription_resync_required("sub", 8, 4)
        .await
        .is_err());
    drop(fault);
    let keys = vec![("testdb".to_string(), "main".to_string())];
    let before_rows = super::super::catalogue::rows(&shamir, "subscriptions")
        .await
        .unwrap();
    let row = before_rows
        .iter()
        .find(|r| r.get("name").and_then(|v| v.as_str()) == Some("sub"))
        .unwrap();
    let generation = row
        .get("binding_generation")
        .and_then(|v| v.as_u64())
        .unwrap();
    let old_claims = shamir
        .resync_quarantine
        .owner_claims_for(&format!("sub:{generation}"), &keys);
    assert!(!old_claims.is_empty());
    let b_claim = shamir
        .resync_quarantine
        .publish([(keys[0].clone(), "B pending".into())], "sub:other:7");
    super::quarantine_reads_tests::recovery::install_replacement_image(&shamir).await;
    super::quarantine_reads_tests::recovery::rebind_to_replacement(&shamir, "sub", "prof").await;
    let after_rows = super::super::catalogue::rows(&shamir, "subscriptions")
        .await
        .unwrap();
    let row = after_rows
        .iter()
        .find(|r| r.get("name").and_then(|v| v.as_str()) == Some("sub"))
        .unwrap();
    let generation2 = row
        .get("binding_generation")
        .and_then(|v| v.as_u64())
        .unwrap();
    assert!(generation2 > generation);
    shamir.set_replication_resume_probe(super::quarantine_reads_tests::recovery::probe());
    let mut batch = Batch::new();
    batch.id(15);
    batch
        .try_op("resume", alter_subscription("sub").resume())
        .unwrap();
    shamir
        .execute("testdb", &batch.to_request_via_msgpack())
        .await
        .unwrap();
    assert!(shamir
        .resync_quarantine
        .owner_claims_for(&format!("sub:{generation}"), &keys)
        .is_empty());
    assert_eq!(
        shamir
            .resync_quarantine
            .owner_claims_for("sub:other:7", &keys),
        vec![b_claim.clone()]
    );
    assert_refused(&shamir, "testdb", "main").await;
    shamir
        .resync_quarantine
        .rollback_pending(&b_claim, keys.clone());
    assert_serves_named_row(&shamir, "testdb", "main", "replacement").await;
}

/// A SetProfile-only mutation must preserve the complete row except for
/// `profile`, both immediately and after reopening the Fjall system store.
/// COUNTERFACTUAL: if SetProfile drops any unrelated field, the fresh complete
/// row comparison below fails; if its persistence is not durable, reopen fails.
#[cfg(unix)]
#[tokio::test]
async fn set_profile_alone_preserves_complete_row_through_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta");
    let repo_path = dir.path().join("main");
    let before;
    {
        let shamir = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
            .await
            .unwrap();
        let db = shamir.create_db("testdb").await.expect("create_db");
        // RAW fjall (no MemBuffer): this test binds a SUBSCRIPTION to
        // this repo, and the CREATE/RESUME admission
        // (validate_repo_wide_atomic_batch) refuses MemBuffer-wrapped
        // destinations with `atomic_batch_unsupported`. Raw fjall is also
        // the backend the seed writes through, so the durable directory
        // survives the reopen below.
        db.add_repo(RepoConfig::new(
            "main",
            BoxRepoFactory::fjall_raw(repo_path),
        ))
        .await
        .unwrap();
        let mut b = Batch::new();
        b.id(2);
        for profile in ["prof_a", "prof_b"] {
            b.op(
                profile,
                replication_profile(profile).stream(
                    repl_scope("testdb").repo("main").build(),
                    ReplDirection::Pull,
                    ReplMode::ReadOnly,
                ),
            );
        }
        b.op(
            "sub",
            subscription("sub", "tcp://peer:9000", "pub", "prof_a"),
        );
        shamir
            .execute("testdb", &b.to_request_via_msgpack())
            .await
            .unwrap();
        before = super::super::catalogue::rows(&shamir, "subscriptions")
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.get("name").and_then(|v| v.as_str()) == Some("sub"))
            .expect("created subscription row");
        let mut expected = before.clone();
        let QueryValue::Map(fields) = &mut expected else {
            panic!("subscription row must be a map")
        };
        fields.insert("profile".into(), QueryValue::Str("prof_b".into()));
        let mut change = Batch::new();
        change.id(3);
        change
            .try_op("alt", alter_subscription("sub").set_profile("prof_b"))
            .unwrap();
        shamir
            .execute("testdb", &change.to_request_via_msgpack())
            .await
            .unwrap();
        let fresh = super::super::catalogue::rows(&shamir, "subscriptions")
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.get("name").and_then(|v| v.as_str()) == Some("sub"))
            .expect("SetProfile row");
        assert_eq!(fresh, expected, "SetProfile may change profile only");
    }
    let reopened = reopen_durable(sys_path).await;
    let fresh = super::super::catalogue::rows(&reopened, "subscriptions")
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.get("name").and_then(|v| v.as_str()) == Some("sub"))
        .expect("reopened SetProfile row");
    let mut expected = before;
    let QueryValue::Map(fields) = &mut expected else {
        panic!("subscription row must be a map")
    };
    fields.insert("profile".into(), QueryValue::Str("prof_b".into()));
    assert_eq!(
        fresh, expected,
        "complete SetProfile row must survive reopen"
    );
}

/// Pause and SetProfile persist through the same true-replace helper.
/// COUNTERFACTUAL: staged DELETE+merge SET aborts ALTER; Resume marker-removal regression above catches omitted-field merging.
#[cfg(unix)]
#[tokio::test]
async fn pause_and_set_profile_persist_exact_durable_row() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta");
    let repo_path = dir.path().join("main");
    {
        let shamir = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
            .await
            .unwrap();
        let db = shamir.create_db("testdb").await.expect("create_db");
        db.add_repo(RepoConfig::new(
            "main",
            // RAW fjall (no MemBuffer): this test binds a SUBSCRIPTION to
            // this repo, and the CREATE/RESUME admission
            // (validate_repo_wide_atomic_batch) refuses MemBuffer-wrapped
            // destinations with `atomic_batch_unsupported`. Raw fjall is also
            // the backend the seed writes through, so the durable directory
            // survives the reopen below.
            BoxRepoFactory::fjall_raw(repo_path.clone()),
        ))
        .await
        .unwrap();
        db.add_repo(RepoConfig::new("side", BoxRepoFactory::in_memory()))
            .await
            .unwrap();
        // TWO single-stream profiles over the SAME destination (the SetProfile
        // guard requires an identical Pull destination set).
        let mut b = Batch::new();
        b.id(2);
        b.op(
            "prof_a",
            replication_profile("prof_a").stream(
                repl_scope("testdb").repo("main").build(),
                ReplDirection::Pull,
                ReplMode::ReadOnly,
            ),
        );
        b.op(
            "prof_b",
            replication_profile("prof_b").stream(
                repl_scope("testdb").repo("main").build(),
                ReplDirection::Pull,
                ReplMode::ReadOnly,
            ),
        );
        b.op(
            "sub",
            subscription("sub", "tcp://peer:9000", "pub", "prof_a"),
        );
        shamir
            .execute("testdb", &b.to_request_via_msgpack())
            .await
            .unwrap();

        // SetProfile: the durable row's `profile` flips, everything else stays.
        let mut b = Batch::new();
        b.id(3);
        b.try_op("alt", alter_subscription("sub").set_profile("prof_b"))
            .unwrap();
        shamir
            .execute("testdb", &b.to_request_via_msgpack())
            .await
            .unwrap();
        let rows = super::super::catalogue::rows(&shamir, "subscriptions")
            .await
            .unwrap();
        let row = rows
            .iter()
            .find(|r| r.get("name").and_then(|v| v.as_str()) == Some("sub"))
            .expect("row must survive SetProfile");
        assert_eq!(row.get("profile").and_then(|v| v.as_str()), Some("prof_b"));
        assert_eq!(row.get("state").and_then(|v| v.as_str()), Some("active"));
        assert_eq!(row.get("quarantined"), None);
        assert_eq!(row.get("status_message"), None);

        // Pause: the durable row's `state` flips to `paused`.
        let mut b = Batch::new();
        b.id(4);
        b.try_op("alt", alter_subscription("sub").pause()).unwrap();
        shamir
            .execute("testdb", &b.to_request_via_msgpack())
            .await
            .unwrap();
        let rows = super::super::catalogue::rows(&shamir, "subscriptions")
            .await
            .unwrap();
        let row = rows
            .iter()
            .find(|r| r.get("name").and_then(|v| v.as_str()) == Some("sub"))
            .expect("row must survive Pause");
        assert_eq!(row.get("state").and_then(|v| v.as_str()), Some("paused"));
        assert_eq!(row.get("profile").and_then(|v| v.as_str()), Some("prof_b"));
        assert_eq!(row.get("quarantined"), None);
        assert_eq!(row.get("status_message"), None);
        assert_eq!(row.get("resync_from_receipt"), None);
    }
    let reopened = reopen_durable(sys_path).await;
    let rows = super::super::catalogue::rows(&reopened, "subscriptions")
        .await
        .unwrap();
    let row = rows
        .iter()
        .find(|r| r.get("name").and_then(|v| v.as_str()) == Some("sub"))
        .expect("paused SetProfile row must survive reopen");
    assert_eq!(row.get("state").and_then(|v| v.as_str()), Some("paused"));
    assert_eq!(row.get("profile").and_then(|v| v.as_str()), Some("prof_b"));
    assert_eq!(row.get("quarantined"), None);
    assert_eq!(row.get("status_message"), None);
    assert_eq!(row.get("resync_from_receipt"), None);
}
