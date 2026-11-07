//! S5c-X round 4 (mechanism 2) — the per-subscription generation lease and
//! its quiesce barrier, tested directly on the db side.
//!
//!   * T3a — a failed Pause flush (seed-catalogue fault) leaves the LIVE row
//!     `paused` (the set committed before the failed flush) AND the lease
//!     invalid: the quiesce ran BEFORE the mutation, so no follower may
//!     apply on behalf of the paused binding.
//!   * T4a — the `futures::poll!` proof: the quiesce invalidator is Pending
//!     while an in-flight apply holds the read guard, completes once the
//!     guard drops, and a second quiesce is immediately Ready.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::task::Poll;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{
    alter_subscription, repl_scope, replication_profile, subscription,
};
use shamir_query_types::admin::{ReplDirection, ReplMode};

use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::types::value::QueryValue;
use crate::ShamirDb;

/// In-memory ShamirDb with `testdb/main` carrying a `users` table (mirrors
/// `binding_lifecycle_tests::setup`, trimmed to what these tests touch).
async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await.expect("create_db");
    db.add_repo(
        RepoConfig::new("main", BoxRepoFactory::in_memory())
            .add_table(crate::engine::table::TableConfig::new("users")),
    )
    .await
    .unwrap();
    shamir
}

/// Create ONE Pull profile over `testdb/main` plus an ACTIVE subscription
/// bound to it (builder-only batches).
async fn create_profile_and_subscription(shamir: &ShamirDb) {
    let mut b = Batch::new();
    b.id(2);
    b.op(
        "profile",
        replication_profile("prof").stream(
            repl_scope("testdb").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    b.op("sub", subscription("sub", "tcp://peer:9000", "pub", "prof"));
    shamir
        .execute(
            "testdb",
            &b.to_request_via_msgpack().expect("batch request encoding"),
        )
        .await
        .unwrap();
}

/// Raw read of one subscription row (mirrors
/// `binding_lifecycle_tests::subscription_row`: status/list projects only a
/// subset of fields, so the durable table is read directly).
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

/// Simulate a running follower: issue the lease for the row's CURRENT
/// durable binding generation (under the control lock, like the supervisor).
async fn take_lease(shamir: &ShamirDb) -> (u64, Arc<crate::repl_binding::SubscriptionLease>) {
    let gen = subscription_row(shamir, "sub")
        .await
        .get("binding_generation")
        .and_then(|v| v.as_u64())
        .expect("row carries a binding generation");
    let g = shamir.lock_replication_control().await;
    let lease = shamir
        .take_subscription_lease("sub", gen, &g)
        .await
        .expect("no valid lease exists yet; the fresh lease must be issued");
    drop(g);
    (gen, lease)
}

/// T3a: the db-side half of the Pause semantics. The ALTER pause batch
/// quiesces the lease BEFORE mutating the row, then flushes — so when the
/// flush fault fails the batch, the row is already durably `paused` AND the
/// lease is invalid (a live follower cannot apply another event).
#[tokio::test]
async fn pause_flush_failure_leaves_live_row_paused_and_lease_invalid() {
    let shamir = setup().await;
    create_profile_and_subscription(&shamir).await;

    let (_gen, lease) = take_lease(&shamir).await;
    assert!(lease.valid.load(Ordering::Acquire));

    let fault = crate::shamir_db::shamir_db::SeedCatalogueFlushFaultForTest::arm(&shamir);
    let mut b = Batch::new();
    b.id(3);
    b.try_op("pause", alter_subscription("sub").pause())
        .unwrap();
    let err = shamir
        .execute(
            "testdb",
            &b.to_request_via_msgpack().expect("batch request encoding"),
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("seed-catalogue"),
        "the injected flush fault must surface: {err}"
    );

    // The set committed before the failed flush: the live row reads paused…
    let row = subscription_row(&shamir, "sub").await;
    assert_eq!(
        row.get("state").and_then(|v| v.as_str()),
        Some("paused"),
        "the pause set must commit before the failed flush"
    );
    // …and the quiesce invalidated the lease BEFORE the mutation.
    assert!(
        !lease.valid.load(Ordering::Acquire),
        "the lease must be quiesced before the pause mutation"
    );
    drop(fault);
}

/// T4a: the quiesce primitive. While an in-flight apply holds the lease's
/// read guard, the invalidator polls PENDING (`futures::poll!` — no sleeps,
/// no yields); the lease is already invalid at that point, so no NEW apply
/// can start. Once the guard drops, the quiesce completes, and a second
/// quiesce polls READY immediately (no holders; the gate is not poisoned).
#[tokio::test]
async fn quiesce_waits_for_in_flight_apply_and_blocks_new_ones() {
    let shamir = setup().await;
    create_profile_and_subscription(&shamir).await;

    let (_gen, lease) = take_lease(&shamir).await;

    // A task holding the read guard, exactly like the follower loop's apply.
    let held = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let apply_task = {
        let lease = lease.clone();
        let held = held.clone();
        let release = release.clone();
        tokio::spawn(async move {
            let g = lease.apply_gate.read().await;
            held.notify_one();
            release.notified().await;
            drop(g);
        })
    };
    held.notified().await; // the read guard is held

    let control = shamir.lock_replication_control().await;
    let mut q = Box::pin(shamir.quiesce_subscription_lease_locked("sub", &control));
    assert!(
        matches!(futures::poll!(q.as_mut()), Poll::Pending),
        "the invalidator must stay Pending while an apply holds the read guard"
    );
    assert!(
        !lease.valid.load(Ordering::Acquire),
        "the lease must already be invalid while the quiesce drains"
    );

    release.notify_one();
    apply_task.await.unwrap();
    q.await; // drains now: the in-flight apply finished

    // A second quiesce polls READY immediately — no holders, gate healthy.
    let mut q2 = Box::pin(shamir.quiesce_subscription_lease_locked("sub", &control));
    assert!(
        matches!(futures::poll!(q2.as_mut()), Poll::Ready(_)),
        "a second quiesce with no holders must complete immediately"
    );
}
