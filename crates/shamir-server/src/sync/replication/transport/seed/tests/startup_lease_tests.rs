//! Round-6 item 3 — RAII startup lease. Every early exit or CANCELLATION
//! after lease issuance must invalidate the lease (`StartupLeaseGuard`);
//! without it a valid lease is stranded with no follower and every later
//! `take_subscription_lease` is refused forever.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use futures_util::poll; // poll! re-export
use shamir_db::ShamirDb;
use shamir_query_builder::ddl::{drop_replication_profile, repl_scope, replication_profile};
use shamir_query_types::admin::{ReplDirection, ReplMode};
use tokio::sync::Notify;

use crate::replication::supervisor::Subscription;

use super::lease_gate_tests::{
    build_db, create_profile_and_subscription, leader_version, run_op, sub_generation, supervisor,
    wait_until_lease_gone, write_rows,
};

/// COUNTERFACTUAL (round-6 item 3): without the RAII guard the
/// profile-resolution failure AFTER lease issuance strands a VALID lease —
/// the first-half `wait_until_lease_gone` panics, and the repaired
/// reconcile#3 is REFUSED by W1's valid-lease semantics (bookmark stays 0) →
/// the second-half assertion is red too.
///
/// Scenario: reconcile#1 starts sub_a cleanly (bounded loop#1, joined);
/// the profile row is then deleted, so reconcile#2 takes a fresh lease,
/// passes the barrier, and FAILS in `resolve_profile_streams` — after
/// issuance. The guard must clean the lease up; the repaired profile lets
/// reconcile#3 start for real.
#[tokio::test]
async fn profile_resolution_failure_after_issuance_cleans_the_lease() {
    let leader = build_db().await;
    write_rows(&leader, 0, 2).await;
    let version = leader_version(&leader).await;

    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;

    // reconcile#1: legit start under lease L1; the bounded loop exits and
    // its teardown invalidates L1.
    let sup1 = supervisor(follower.clone(), leader.clone()).with_max_iterations(1);
    sup1.reconcile().await;
    sup1.wait_for_subscription("sub_a").await;

    // Delete the profile row: reconcile#2's `resolve_profile_streams` now
    // fails AFTER its lease was issued (the start returns Err).
    run_op(&follower, drop_replication_profile("prof_a")).await;
    let sup2 = supervisor(follower.clone(), leader.clone()).with_max_iterations(1);
    sup2.reconcile().await; // start fails; nothing spawns

    assert!(
        !sup2.is_running("sub_a"),
        "nothing may spawn on a failed start"
    );
    // The stranded lease MUST be gone: bounded wait for the guard's detached
    // cleanup task (Drop cannot await, it spawns).
    wait_until_lease_gone(&follower, "sub_a").await;
    assert_eq!(
        follower
            .subscription_lease("sub_a")
            .map(|l| l.valid.load(Ordering::Acquire)),
        None,
        "the failed start's lease must be invalidated, not stranded"
    );

    // Repair the profile: the next reconcile STARTS and catches up.
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id(1);
    b.op(
        "cp",
        replication_profile("prof_a").stream(
            repl_scope("app").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    follower
        .execute("app", &b.to_request_via_msgpack())
        .await
        .expect("recreate prof_a");
    let sup3 = supervisor(follower.clone(), leader.clone()).with_max_iterations(1);
    sup3.reconcile().await;
    sup3.wait_for_subscription("sub_a").await;
    assert!(
        bookmark_(&follower).await >= version,
        "the repaired reconcile must start the subscription"
    );
}

/// Follower `app/main` bookmark (local alias to keep the import list tight).
async fn bookmark_(follower: &ShamirDb) -> u64 {
    follower
        .get_db("app")
        .and_then(|d| d.get_repo("main"))
        .expect("follower repo")
        .replication_bookmark()
        .await
        .expect("bookmark")
}

/// COUNTERFACTUAL (round-6 item 3): without the RAII guard, DROPPING the
/// in-flight `start_subscription` future (cancellation after issuance)
/// leaves `Some(true)` forever — `wait_until_lease_gone` panics → red.
#[tokio::test]
async fn cancelled_startup_future_cleans_the_lease() {
    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;

    // Hook: signals entry, then parks on a Notify that never fires — the
    // start future stays pending AFTER the lease was issued (the hook runs
    // after the lease-take and barrier flush, before spawn).
    let entered = Arc::new(AtomicBool::new(false));
    let hold = Arc::new(Notify::new());
    let (entered_h, hold_h) = (entered.clone(), hold.clone());
    let sup =
        supervisor(follower.clone(), build_db().await).with_start_race_hook(Arc::new(move || {
            let entered = entered_h.clone();
            let hold = hold_h.clone();
            Box::pin(async move {
                entered.store(true, Ordering::Release);
                hold.notified().await; // parks the start forever
            })
        }));

    let g1_sub = Subscription {
        name: "sub_a".to_string(),
        upstream: "tcp://leader:9000".to_string(),
        publication: "pub_a".to_string(),
        profile: "prof_a".to_string(),
        state: "active".to_string(),
        binding_generation: sub_generation(&follower, "sub_a").await,
        binding_destinations: vec![("app".to_string(), "main".to_string())],
    };

    // Drive the start as a future; poll until the hook is entered (the lease
    // already exists and is valid at that point).
    let mut start = Box::pin(sup.start_subscription(&g1_sub));
    // `poll!` expands to code containing `.await`, so it cannot run inside
    // the plain closure `wait_until` takes (E0728). Poll the future here in
    // the async fn instead, yielding between attempts, until the hook is
    // entered (the lease already exists and is valid at that point).
    for _ in 0..10_000 {
        if entered.load(Ordering::Acquire) {
            break;
        }
        let _ = poll!(start.as_mut());
        tokio::task::yield_now().await;
    }
    assert!(
        entered.load(Ordering::Acquire),
        "the start hook must be entered within the yield budget"
    );
    assert_eq!(
        follower
            .subscription_lease("sub_a")
            .map(|l| l.valid.load(Ordering::Acquire)),
        Some(true),
        "the lease must be issued (and valid) when the hook is entered"
    );

    // CANCEL: drop the in-flight start future → the guard's Drop runs on the
    // executor thread and spawns the detached cleanup.
    drop(start);
    wait_until_lease_gone(&follower, "sub_a").await;
    assert!(!sup.is_running("sub_a"), "nothing may have spawned");
}
