//! Round 6 item 5 — a pending quarantine key is removed ONLY by an explicit
//! confirmation after the owning mark's SET + interner persist + flush +
//! refresh ALL succeeded (or a definitive rollback). A refresh triggered by
//! an UNRELATED subscription must never clear it.

use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::shamir_db::shamir_db::TerminalRowSetFaultForTest;
use crate::types::value::QueryValue;
use crate::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{drop_subscription, repl_scope, replication_profile, subscription};
use shamir_query_builder::Query;
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_query_types::batch::BatchError;

/// In-memory ShamirDb with `testdb/{main,side}`, each with a `users` table.
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
    shamir
}

async fn read_users(shamir: &ShamirDb, repo: &str) -> Result<(), BatchError> {
    let mut b = Batch::new();
    b.id(9);
    b.query("r", Query::with_repo(repo, "users"));
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .map(|_| ())
}

/// Create a Pull profile over `destinations` + an ACTIVE subscription.
async fn create_subscription(shamir: &ShamirDb, sub: &str, profile: &str, repo: &str) {
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
    b.op("sub", subscription(sub, "tcp://peer:9000", "pub", profile));
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
}

/// The pending-protection lifecycle (round 6 item 5):
///
/// 1. Arm the SET-fault seam and mark `sub` terminal: the cache keys are
///    PREPUBLISHED, then the row SET fails BEFORE the row changes — the
///    durable row stays healthy but the pending key must already protect.
/// 2. Reads of the covered destination AND its DROP/RENAME guards are
///    refused while pending.
/// 3. Marking an UNRELATED subscription runs a full refresh — the pending
///    key must SURVIVE it (refresh is not a persistence acknowledgement).
/// 4. Disarm, re-mark `sub` successfully (SET + interner + flush + refresh
///    all Ok → `confirm_pending`), then clear the row: the destination
///    serves again, proving the pending set was confirmed (not merely
///    retained).
///
/// COUNTERFACTUAL: reverting `ResyncQuarantineCache::install` to the
/// pre-round-6 behavior of dropping unmatched pending keys turns step 2/3
/// red — the unrelated subscription's refresh would silently drop the
/// failed mark's prepublished key and unblock a destination whose
/// quarantine never durably landed. Reverting the SET-fault seam check (or
/// its `confirm_pending` call) breaks steps 1 and 4 respectively.
#[tokio::test]
async fn pending_quarantine_survives_unrelated_refresh_until_confirmation() {
    let shamir = setup().await;
    create_subscription(&shamir, "sub", "prof", "main").await;
    create_subscription(&shamir, "sub_unrelated", "prof2", "side").await;

    // Step 1: publish (prepublish) happens, then the row SET fails.
    let fault = TerminalRowSetFaultForTest::arm(&shamir);
    assert!(
        shamir
            .mark_subscription_resync_required("sub", 7, 3)
            .await
            .is_err(),
        "the injected row-SET fault must fail the terminal mark"
    );
    drop(fault); // RAII disarm — the fault fired exactly once.

    // Step 2: pending protection WITHOUT durability — reads refused …
    let err = read_users(&shamir, "main")
        .await
        .expect_err("the pending quarantine key must refuse reads of the covered destination");
    assert_eq!(err.code(), Some("resync_required"), "{err}");
    // … DROP SUBSCRIPTION refused (all bound destinations are consulted) …
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
        "the pending key must refuse DROP SUBSCRIPTION: {err}"
    );
    // … and destination RENAME refused (pending branch of the binding guard).
    let err = shamir
        .rename_repo_as(
            "testdb",
            "main",
            "main2",
            shamir_types::access::Actor::System,
        )
        .await
        .unwrap_err();
    assert_eq!(
        err.code(),
        Some("replication_destination_bound"),
        "the pending key must refuse destination RENAME: {err}"
    );

    // Step 3: an UNRELATED subscription's successful terminal mark runs a
    // full refresh — the pending key MUST survive it.
    shamir
        .mark_subscription_resync_required("sub_unrelated", 5, 2)
        .await
        .unwrap();
    let err = read_users(&shamir, "main")
        .await
        .expect_err("the unrelated refresh must NOT clear the pending key");
    assert_eq!(err.code(), Some("resync_required"), "{err}");

    // Step 4: re-mark `sub` with the seam disarmed — full success confirms
    // the pending keys. Prove confirmation by clearing the row (durable
    // replace, same helper the Resume arm uses) and refreshing: NO pending
    // key may keep the destination blocked once the row no longer derives
    // it.
    shamir
        .mark_subscription_resync_required("sub", 7, 3)
        .await
        .unwrap();
    let table = shamir.system_store().subscriptions_table().await.unwrap();
    let interner = table.interner_for_test().get().await.unwrap();
    let refs = crate::types::common::new_map();
    let ctx = crate::query::filter::FilterContext::new(interner, &refs);
    let mut row = table
        .read(
            &crate::query::read::ReadQuery::new("subscriptions").filter(
                crate::query::filter::Filter::Eq {
                    field: vec!["name".to_string()],
                    value: crate::query::filter::FilterValue::String("sub".to_string()),
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
        .replace_subscription_row(&table, "sub".to_string(), row)
        .await
        .unwrap();
    shamir.refresh_resync_quarantine_cache().await.unwrap();
    read_users(&shamir, "main")
        .await
        .expect("a confirmed-and-cleared pending key must leave the destination serving");
}
