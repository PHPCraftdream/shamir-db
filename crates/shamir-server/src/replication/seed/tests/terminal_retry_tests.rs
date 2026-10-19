//! Round-6 item 2 — terminal marking retries guard on GENERATION currency,
//! not `lease.valid`. The B3 test moved here from `lease_gate_tests.rs` and
//! was REWRITTEN: the injected flush failures now hit TERMINAL persistence
//! (armed from inside the `start_race_hook`, i.e. after the start barrier,
//! while the marking runs) — not the start barriers.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use shamir_db::shamir_db::shamir_db::SeedCatalogueFlushFaultForTest;
use shamir_query_types::wire::repl::ReplResponse;

use crate::replication::error::ReplError;
use crate::replication::in_process::InProcessReplSource;
use crate::replication::source::ReplSource;
use crate::replication::supervisor::{ReplSourceFactory, Subscription, SubscriptionSupervisor};

use super::lease_gate_tests::{
    bookmark, build_db, create_profile_and_subscription, leader_version, sub_state,
    wait_until_bookmark_covers, write_rows,
};

/// Source whose pull reply carries a WRONG repo lineage (identity fence
/// trips `IdentityMismatch` on every pull; hello delegates untouched) — the
/// epoch-order recipe from `identity_fence/tests/epoch_order_tests.rs`.
struct WrongIdentitySource {
    inner: InProcessReplSource,
    bad_pull: ReplResponse,
}

#[async_trait]
impl ReplSource for WrongIdentitySource {
    async fn hello(
        &self,
        node_id: &str,
    ) -> Result<shamir_query_types::wire::repl::ReplResponse, ReplError> {
        self.inner.hello(node_id).await
    }

    async fn pull(
        &self,
        _db: &str,
        _repo: &str,
        _from_version: u64,
        _limit: u32,
        _wait_ms: Option<u32>,
    ) -> Result<shamir_query_types::wire::repl::ReplResponse, ReplError> {
        Ok(self.bad_pull.clone())
    }
}

/// COUNTERFACTUAL (round-6 item 2): reverting `mark_with_retry`'s currency
/// guard to the old `lease.valid` check makes this test RED — the terminal
/// mark itself quiesces/invalidates the subscription's lease FIRST
/// (`admin_replication.rs::mark_subscription_terminal_generation` →
/// `quiesce_subscription_lease_locked`), so with the old guard attempt 2
/// sees `valid == false` and exits immediately; the row never lands
/// `resync_required` and the final state/coverage assertions go red. The
/// generation-currency guard reads the CURRENT subscription row, which the
/// mark's own quiesce never touches.
///
/// Flush-budget arithmetic (`arm_fail_first(2)` armed INSIDE the
/// `start_race_hook`, which fires AFTER the barrier flush and BEFORE spawn):
/// reconcile#2's barrier flush passes uncounted; loop#2 trips the identity
/// fence on its first pull (no flush); marking attempt 1 consumes fault #1,
/// attempt 2 consumes fault #2, attempt 3's flush passes → the mark lands.
/// Exactly 2 faults, 3 mark flushes, none left over.
///
/// Determinism: `tokio::time::pause` (start_paused) auto-advances the
/// production retry sleeps on the current-thread runtime; the fence trips on
/// the mutated pull BEFORE decode/apply.
#[tokio::test(start_paused = true)]
async fn terminal_marking_retries_until_success() {
    let leader = build_db().await;
    write_rows(&leader, 0, 2).await;
    let version = leader_version(&leader).await;

    // Build the WRONG-identity pull reply from the real leader (identity
    // regression via the source — the epoch_order recipe).
    let inner = InProcessReplSource::new((*leader).clone());
    let mut bad_pull = inner.pull("app", "main", 1, 1000, Some(0)).await.unwrap();
    let expected = leader
        .get_db("app")
        .unwrap()
        .get_repo("main")
        .unwrap()
        .repo_identity()
        .await
        .unwrap();
    if let ReplResponse::Pull {
        repo_lineage_id, ..
    } = &mut bad_pull
    {
        let mut observed = expected;
        observed.lineage_id[0] ^= 1;
        *repo_lineage_id = Some(observed.lineage_id);
    }

    // The factory hands loop#1 the HONEST source and every later start the
    // WRONG-identity source.
    let counter = Arc::new(AtomicUsize::new(0));
    let factory: ReplSourceFactory = {
        let leader = leader.clone();
        let bad_pull = bad_pull.clone();
        let counter = counter.clone();
        Arc::new(move |_sub: &Subscription| {
            if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                Arc::new(InProcessReplSource::new((*leader).clone())) as Arc<dyn ReplSource>
            } else {
                Arc::new(WrongIdentitySource {
                    inner: InProcessReplSource::new((*leader).clone()),
                    bad_pull: bad_pull.clone(),
                }) as Arc<dyn ReplSource>
            }
        })
    };

    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;
    let sup =
        SubscriptionSupervisor::new(follower.clone(), factory, "follower-1").with_max_iterations(1);

    // loop#1: applies v1, exits, self-deregisters, lease invalidated.
    sup.reconcile().await;
    sup.wait_for_subscription("sub_a").await;
    wait_until_bookmark_covers(&follower, "main", version).await;

    // Reconcile#2: the start_race_hook arms the REAL catalogue-flush seam
    // when it fires (after the barrier flush, before spawn). The RAII fault
    // guard is parked in a captured `Mutex<Option<_>>` and dropped only
    // AFTER the assertions, so the seam stays armed while loop#2 runs.
    let fault_slot: Arc<Mutex<Option<SeedCatalogueFlushFaultForTest>>> = Arc::new(Mutex::new(None));
    let hook_follower = follower.clone();
    let hook_slot = fault_slot.clone();
    let sup2 = SubscriptionSupervisor::new(
        follower.clone(),
        {
            let leader = leader.clone();
            let bad_pull = bad_pull.clone();
            Arc::new(move |_sub: &Subscription| {
                Arc::new(WrongIdentitySource {
                    inner: InProcessReplSource::new((*leader).clone()),
                    bad_pull: bad_pull.clone(),
                }) as Arc<dyn ReplSource>
            })
        },
        "follower-2",
    )
    .with_max_iterations(1)
    .with_start_race_hook(Arc::new(move || {
        let follower = hook_follower.clone();
        let slot = hook_slot.clone();
        Box::pin(async move {
            *slot.lock().expect("fault slot") =
                Some(SeedCatalogueFlushFaultForTest::arm_fail_first(&follower, 2));
        })
    }));

    // Barrier passes (the hook arms the seam AFTER it); loop#2 spawns and
    // trips the identity fence; the wrapper's marking fails twice at its
    // flush seam and succeeds on attempt 3.
    sup2.reconcile().await;
    sup2.wait_for_subscription("sub_a").await; // joins loop#2 + its marking

    // Release the seam only after the assertions (drop the RAII fault guard).
    *fault_slot.lock().expect("fault slot") = None;

    assert_eq!(
        sub_state(&follower, "sub_a").await.as_deref(),
        Some("resync_required"),
        "the retried terminal marking must have landed"
    );
    assert!(
        follower
            .has_resync_required_subscription_covering("app", "main")
            .await
            .unwrap(),
        "the quarantine must cover app/main"
    );
    // Loop#1's honest application is untouched by loop#2's fence trip.
    assert!(bookmark(&follower, "main").await >= version);
}
