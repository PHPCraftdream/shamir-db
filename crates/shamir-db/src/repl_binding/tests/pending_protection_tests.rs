//! Round 6 item 5 + round 7 item 5 + round 8 item 5 — a pending quarantine key is removed
//! ONLY by an explicit confirmation/rollback of its OWN
//! `name:generation` owner (after the owning mark's SET + interner persist
//! + flush + refresh ALL succeeded, or a definitive rollback).
//!
//! A refresh triggered by an UNRELATED subscription — or that subscription's
//! successful Resume — must never clear another owner's pending claim.

use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::shamir_db::shamir_db::TerminalRowSetFaultForTest;
use crate::types::value::QueryValue;
use crate::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{
    alter_subscription, drop_subscription, repl_scope, replication_profile, subscription,
};
use shamir_query_builder::Query;
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_query_types::batch::BatchError;

/// In-memory ShamirDb with `testdb/{main,side}`, each with a `users` table.
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

/// Create a Pull profile over ONE destination (`matches_scope` admission
/// rule: `catalogue.rs::matches_scope` requires EXACTLY one stream) + an
/// ACTIVE subscription.
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

/// The pending-protection lifecycle (round 6 item 5, owner-scoped round 7
/// item 5). `sub_a` and `sub_b` are BOTH bound to the SAME ordinary
/// destination `testdb/main`:
///
/// 1. Arm the SET-fault seam and mark `sub_a` terminal: the cache key is
///    PREPUBLISHED under owner `sub_a:<generation>`, then the row SET fails
///    BEFORE the row changes — the durable row stays healthy but the
///    pending claim must already protect.
/// 2. Reads of the covered destination AND its DROP/RENAME guards are
///    refused while pending.
/// 3. `sub_b` (same destination, paused) completes a NORMAL Resume
///    successfully: its rollback claims owner `sub_b:<generation>` only —
///    `sub_a`'s pending claim SURVIVES the unrelated rollback AND the
///    refresh the Resume runs.
/// 4. Dropping the (healthy) `sub_b` runs another full refresh — the claim
///    still survives.
/// 5. Disarm, re-mark `sub_a` successfully (SET + interner + flush + refresh
///    all Ok → `confirm_pending` under `sub_a:<generation>`), then clear the
///    row: the destination serves again, proving the pending set was
///    confirmed (not merely retained).
///
/// COUNTERFACTUAL: reverting to pre-round-7 BARE-KEY rollback (a Resume
/// rolls back ALL of that destination's pending keys) turns step 3 red —
/// `sub_b`'s successful Resume would erase `sub_a`'s failed mark's pending
/// protection and unblock a destination whose quarantine never durably
/// landed. Reverting `install` to dropping unmatched pending keys turns
/// steps 3/4 red via the refresh; reverting the SET-fault seam check (or
/// its `confirm_pending` call) breaks steps 1 and 5 respectively.
#[tokio::test]
async fn pending_protection_survives_another_subscription_resume() {
    let shamir = setup().await;
    create_subscription(&shamir, "sub_a", "prof_a", "main").await;
    create_subscription(&shamir, "sub_b", "prof_b", "main").await;
    create_subscription(&shamir, "sub_c", "prof_c", "side").await;
    // `sub_b` must be RESUMABLE later: pause it first (a plain, non-seed
    // Resume over a paused row passes `validate_seed_resume` — the repo has
    // no seed receipt, so the per-stream check is vacuous).
    let mut b = Batch::new();
    b.id(4);
    b.try_op("pause", alter_subscription("sub_b").pause())
        .unwrap();
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Step 1: publish (prepublish) happens, then the row SET fails.
    let fault = TerminalRowSetFaultForTest::arm(&shamir);
    assert!(
        shamir
            .mark_subscription_resync_required("sub_a", 7, 3)
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
    b.op(
        "drop",
        drop_subscription("sub_a")
            .hmac("test")
            .build()
            .expect("build op"),
    );
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

    // Step 3: `sub_b` completes a NORMAL Resume successfully (its own
    // rollback touches only owner `sub_b:<generation>`); `sub_a`'s pending
    // claim must keep the shared destination blocked.
    let mut b = Batch::new();
    b.id(5);
    b.try_op("resume", alter_subscription("sub_b").resume())
        .unwrap();
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    let err = read_users(&shamir, "main")
        .await
        .expect_err("sub_b's successful Resume must NOT clear sub_a's pending claim");
    assert_eq!(err.code(), Some("resync_required"), "{err}");
    // The guards stay closed too: DROP of `sub_a` and destination RENAME.
    let mut b = Batch::new();
    b.id(7);
    b.op(
        "drop",
        drop_subscription("sub_a")
            .hmac("test")
            .build()
            .expect("build op"),
    );
    let err = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap_err();
    assert_eq!(
        err.code(),
        Some("replication_destination_quarantined"),
        "the pending claim must still refuse DROP SUBSCRIPTION after sub_b's Resume: {err}"
    );
    let err = shamir
        .rename_repo_as(
            "testdb",
            "main",
            "main3",
            shamir_types::access::Actor::System,
        )
        .await
        .unwrap_err();
    assert_eq!(
        err.code(),
        Some("replication_destination_bound"),
        "the pending claim must still refuse destination RENAME after sub_b's Resume: {err}"
    );

    // Step 4 (unrelated-DROP leg): drop `sub_c` bound to testdb/side. This
    // successful DROP runs refresh without touching main's sub_a/sub_b rows.
    let mut b = Batch::new();
    b.id(6);
    b.op(
        "drop",
        drop_subscription("sub_c")
            .hmac("test")
            .build()
            .expect("build op"),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    let err = read_users(&shamir, "main")
        .await
        .expect_err("the refresh after sub_b's DROP must NOT clear sub_a's pending claim");
    assert_eq!(err.code(), Some("resync_required"), "{err}");

    // Step 5: re-mark `sub_a` with the seam disarmed — full success confirms
    // the pending keys under its own owner. Prove confirmation by clearing
    // the row (durable replace, same helper the Resume arm uses) and
    // refreshing: NO pending key may keep the destination blocked once the
    // row no longer derives it.
    shamir
        .mark_subscription_resync_required("sub_a", 7, 3)
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
                    value: crate::query::filter::FilterValue::String("sub_a".to_string()),
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
        .replace_subscription_row(&table, "sub_a".to_string(), row)
        .await
        .unwrap();
    shamir.refresh_resync_quarantine_cache().await.unwrap();
    read_users(&shamir, "main")
        .await
        .expect("a confirmed-and-cleared pending key must leave the destination serving");
}
