//! Round-6 item 4 — multi-target exit: REFERENCE-COUNTED teardown. All
//! target loops of one start share ONE lease; only the LAST exiting loop —
//! the only one guaranteed that no sibling still holds an apply read guard
//! (all loops have returned) — invalidates the lease and removes the handle.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{repl_scope, replication_profile, subscription};
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_query_types::wire::repl::ReplResponse;
use tokio::sync::Notify;

use crate::replication::error::ReplError;
use crate::replication::in_process::InProcessReplSource;
use crate::replication::source::ReplSource;
use crate::replication::supervisor::{ReplSourceFactory, Subscription, SubscriptionSupervisor};

use super::lease_gate_tests::{
    build_db, wait_until, wait_until_deregistered, wait_until_lease_gone, write_rows,
};

/// Two-stream profile `prof_ab` (pull `app/main` + `app/other`) + ACTIVE
/// subscription `sub_a` bound to it — the row's durable binding covers BOTH
/// destinations, so `start_subscription` spawns two loops under one lease.
async fn create_two_target_subscription(follower: &ShamirDb) {
    let mut b = Batch::new();
    b.id(1);
    b.op(
        "cp",
        replication_profile("prof_ab")
            .stream(
                repl_scope("app").repo("main").build(),
                ReplDirection::Pull,
                ReplMode::ReadOnly,
            )
            .stream(
                repl_scope("app").repo("other").build(),
                ReplDirection::Pull,
                ReplMode::ReadOnly,
            ),
    );
    b.op(
        "cs",
        subscription("sub_a", "tcp://leader:9000", "pub_a", "prof_ab"),
    );
    follower
        .execute("app", &b.to_request_via_msgpack())
        .await
        .expect("create two-target subscription");
}

/// Source wrapper dispatching per destination: `app/main` pulls answer
/// honestly ONCE, then serve a REGRESSED leader epoch (the loop's §5.2 epoch
/// fence turns that into `StaleLeaderEpoch`, exiting the loop);
/// `app/other` pulls park on a test-controlled Notify, keeping that loop
/// (and the shared lease's refcount) alive.
struct StaleMainParkedOtherSource {
    inner: InProcessReplSource,
    main_pulls: AtomicUsize,
    main_second_served: Arc<AtomicBool>,
    park: Arc<Notify>,
}

#[async_trait]
impl ReplSource for StaleMainParkedOtherSource {
    async fn hello(&self, node_id: &str) -> Result<ReplResponse, ReplError> {
        self.inner.hello(node_id).await
    }

    async fn pull(
        &self,
        db: &str,
        repo: &str,
        from_version: u64,
        limit: u32,
        wait_ms: Option<u32>,
    ) -> Result<ReplResponse, ReplError> {
        if repo == "main" {
            if self.main_pulls.fetch_add(1, Ordering::SeqCst) == 0 {
                // First contact: honest reply (events exist — the loop
                // applies immediately, no long-poll).
                return self
                    .inner
                    .pull(db, repo, from_version, limit, wait_ms)
                    .await;
            }
            // Second contact: a regressed epoch → StaleLeaderEpoch in the
            // loop (leader epochs start at 1).
            self.main_second_served.store(true, Ordering::Release);
            let mut regressed = self
                .inner
                .pull(db, repo, from_version, limit, Some(0))
                .await?;
            if let ReplResponse::Pull { leader_epoch, .. } = &mut regressed {
                *leader_epoch = 0;
            }
            return Ok(regressed);
        }
        // app/other: park until the test releases us.
        self.park.notified().await;
        self.inner
            .pull(db, repo, from_version, limit, wait_ms)
            .await
    }
}

/// COUNTERFACTUAL (round-6 item 4): reverting to UNCONDITIONAL teardown in
/// the wrapper's exit housekeeping makes the MID-TEST assertions red — the
/// `main` loop's exit would invalidate the shared lease and remove the
/// handle while the `other` loop is still live (mid-test lease goes
/// `Some(true)` → `None`; handle registered → deregistered).
#[tokio::test]
async fn multi_target_exit_refcount_last_exit_removes() {
    let leader = build_db().await;
    // Leader rows BEFORE the start: the `main` loop's first contact applies
    // immediately instead of long-polling (determinism).
    write_rows(&leader, 0, 3).await;

    let follower = build_db().await;
    create_two_target_subscription(&follower).await;

    let main_second_served = Arc::new(AtomicBool::new(false));
    let park = Arc::new(Notify::new());
    let factory: ReplSourceFactory = {
        let leader = leader.clone();
        let served = main_second_served.clone();
        let park = park.clone();
        Arc::new(move |_sub: &Subscription| {
            Arc::new(StaleMainParkedOtherSource {
                inner: InProcessReplSource::new((*leader).clone()),
                main_pulls: AtomicUsize::new(0),
                main_second_served: served.clone(),
                park: park.clone(),
            }) as Arc<dyn ReplSource>
        })
    };
    // Run-forever supervisor: nothing exits on its own.
    let sup = SubscriptionSupervisor::new(follower.clone(), factory, "follower-1")
        .with_poll_wait_ms(2000);

    sup.reconcile().await; // spawns TWO loops under ONE shared lease
    assert_eq!(sup.active_count(), 1, "one handle, two target loops");
    assert_eq!(
        follower
            .subscription_lease("sub_a")
            .map(|l| l.valid.load(Ordering::Acquire)),
        Some(true),
        "both loops share a valid lease"
    );

    // The `main` loop: first contact applies, second contact trips the §5.2
    // epoch fence → StaleLeaderEpoch → the loop exits.
    wait_until(|| main_second_served.load(Ordering::Acquire)).await;
    // Settle budget: after the loop returned, the wrapper's exit housekeeping
    // for StaleLeaderEpoch is synchronous (no marking — it is not a
    // Seed/IdentityMismatch error); these yields let it run.
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }

    // MID-TEST: the live `other` loop holds the refcount — the lease is
    // STILL valid and the handle STILL registered.
    assert_eq!(
        follower
            .subscription_lease("sub_a")
            .map(|l| l.valid.load(Ordering::Acquire)),
        Some(true),
        "the live `other` loop must keep the shared lease valid"
    );
    assert!(
        sup.is_running("sub_a"),
        "the handle must stay registered while a target lives"
    );

    // Release the park, then cancel `other` gracefully (max_iterations is
    // per-supervisor, so stop_all is the bounded end): its exit is the LAST
    // exit and runs the teardown.
    park.notify_one();
    sup.stop_all().await;

    // LAST exit: lease invalidated + handle deregistered (bounded waits for
    // the wrapper's teardown).
    wait_until_lease_gone(&follower, "sub_a").await;
    wait_until_deregistered(&sup, "sub_a").await;
    assert!(!sup.is_running("sub_a"));
}
