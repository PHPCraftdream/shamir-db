//! Round-6 item 2 — terminal marking retries guard on GENERATION currency,
//! not `lease.valid`. The B3 test moved here from `lease_gate_tests.rs` and
//! was REWRITTEN: the injected flush failures now hit TERMINAL persistence
//! (armed from inside the `start_race_hook`, i.e. after the start barrier,
//! while the marking runs) — not the start barriers.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use shamir_db::shamir_db::shamir_db::SeedCatalogueFlushFaultForTest;
#[cfg(unix)]
use shamir_db::{ShamirDb, SystemStoreConfig};
use shamir_query_types::wire::repl::ReplResponse;

use crate::replication::error::ReplError;
use crate::replication::in_process::InProcessReplSource;
use crate::replication::source::ReplSource;
use crate::replication::supervisor::{ReplSourceFactory, Subscription, SubscriptionSupervisor};

#[cfg(unix)]
use super::lease_gate_tests::bookmark;
use super::lease_gate_tests::{
    build_db, create_profile_and_subscription, leader_version, sub_state,
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

/// COUNTERFACTUAL (round-7 item 3): the durable row SET and the quarantine
/// cache publication both PRECEDE the faulted flush
/// (`admin_replication.rs` — cache publish :910, row SET :938-946, flush
/// :960), so after even the FIRST failed flush the row already reads
/// `resync_required` and the coverage already holds — the durable-row /
/// coverage assertions below CANNOT distinguish a retrying mark from a
/// single-attempt one. A single-attempt implementation (e.g. reverting
/// `mark_with_retry`'s retry loop, or its generation-currency guard, which
/// aborts after the first failure) records EXACTLY ONE flush attempt with
/// ONE injected failure — the `attempts == 3` / `failures == 2` seam-counter
/// assertions are what turn that reversion RED. (The round-6 COUNTERFACTUAL
/// claimed the row would remain `active` under reversion — that claim was
/// FALSE, precisely because of the SET-before-flush ordering above.)
///
/// Flush-budget arithmetic (`arm_fail_first(2)` armed INSIDE the
/// `start_race_hook`, which fires AFTER the barrier flush and BEFORE spawn):
/// reconcile#2's barrier flush passes uncounted (the counters are
/// snapshotted right after arming, and every flush between arming and the
/// final read belongs to the marking window — loop#2's fence trip performs
/// no flush); loop#2 trips the identity fence on its first pull (no flush);
/// marking attempt 1 consumes fault #1, attempt 2 consumes fault #2,
/// attempt 3's flush passes → the mark lands. Exactly 2 failures, 3 mark
/// flush attempts, none left over.
///
/// Determinism: `tokio::time::pause` (start_paused) auto-advances the
/// production retry sleeps on the current-thread runtime; the fence trips on
/// the mutated pull BEFORE decode/apply.
/// The durable reopen leg requires the Fjall system-store backend and its
/// platform-specific file locking; the portable retry oracle below exercises
/// the same real supervisor marking path without requiring reopen semantics.
#[tokio::test(start_paused = true)]
async fn terminal_marking_retries_three_flush_attempts_portably() {
    let leader = build_db().await;
    write_rows(&leader, 0, 2).await;
    let version = leader_version(&leader).await;

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
    let sup = SubscriptionSupervisor::new(follower.clone(), factory, "follower-portable")
        .with_max_iterations(1);

    sup.reconcile().await;
    sup.wait_for_subscription("sub_a").await;
    wait_until_bookmark_covers(&follower, "main", version).await;

    let fault_slot: Arc<Mutex<Option<SeedCatalogueFlushFaultForTest>>> = Arc::new(Mutex::new(None));
    let baseline: Arc<Mutex<(usize, usize)>> = Arc::new(Mutex::new((0, 0)));
    let hook_follower = follower.clone();
    let hook_slot = fault_slot.clone();
    let hook_baseline = baseline.clone();
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
        "follower-portable-2",
    )
    .with_max_iterations(1)
    .with_start_race_hook(Arc::new(move || {
        let follower = hook_follower.clone();
        let slot = hook_slot.clone();
        let baseline = hook_baseline.clone();
        Box::pin(async move {
            let guard = SeedCatalogueFlushFaultForTest::arm_fail_first(&follower, 2);
            *baseline.lock().expect("counter baseline") = (guard.attempts(), guard.failures());
            *slot.lock().expect("fault slot") = Some(guard);
        })
    }));

    sup2.reconcile().await;
    sup2.wait_for_subscription("sub_a").await;
    let (attempts, failures, successes) = {
        let slot = fault_slot.lock().expect("fault slot");
        let (base_a, base_f) = *baseline.lock().expect("counter baseline");
        let guard = slot.as_ref().expect("seam still armed");
        (
            guard.attempts() - base_a,
            guard.failures() - base_f,
            guard.successes(),
        )
    };
    assert_eq!(attempts, 3, "2 failed flushes and 1 successful flush");
    assert_eq!(failures, 2, "both injected failures must be consumed");
    assert_eq!(successes, 1, "the terminal mark must complete a real flush");
    *fault_slot.lock().expect("fault slot") = None;
    drop(sup2);
    drop(sup);

    assert_eq!(
        sub_state(&follower, "sub_a").await.as_deref(),
        Some("resync_required")
    );
    assert!(follower
        .has_resync_required_subscription_covering("app", "main")
        .await
        .unwrap());
}

#[cfg(unix)]
async fn persistent_follower(meta: std::path::PathBuf) -> Arc<ShamirDb> {
    let follower = Arc::new(
        ShamirDb::init(SystemStoreConfig::Fjall(meta))
            .await
            .expect("init persistent follower"),
    );
    let owner =
        shamir_db::access::Actor::User(shamir_db::access::principal64_from_username("alice"));
    follower
        .create_db_as("app", owner.clone())
        .await
        .expect("create_db");
    for repo in ["main", "other"] {
        let cfg = shamir_db::engine::repo::RepoConfig::new(
            repo,
            shamir_db::engine::repo::BoxRepoFactory::in_memory(),
        )
        .add_table(shamir_db::engine::table::TableConfig::new("items"));
        follower
            .add_repo_as("app", cfg, owner.clone())
            .await
            .expect("add follower repo");
    }
    follower
}

/// Reopen the persistent follower, retrying the fjall DIRECTORY lock. The lock
/// is held until the LAST clone of the first database's store drops; one of
/// those clones belongs to the drainer's background repo handle, which exits
/// only after its 50 ms liveness tick (`drainer.rs:1666-1687`) observes that the
/// foreground handle is gone. The test runs on a PAUSED clock, where a tick
/// fires only when the runtime is idle and time auto-advances: `yield_now`
/// never advances it (and a running `spawn_blocking` open inhibits advancing),
/// so each retry SLEEPS 50 ms of virtual time between attempts. Only the lock
/// error is retried; any other initialisation failure fails the test at once.
/// (Same precedent as `shamir-engine`'s `reopen_disk_repo` / `open_retry`.)
#[cfg(unix)]
async fn reopen_follower_with_lock_retry(meta: std::path::PathBuf) -> ShamirDb {
    let mut last_lock_error = None;
    for _ in 0..10 {
        match ShamirDb::init(SystemStoreConfig::Fjall(meta.clone())).await {
            Ok(db) => return db,
            Err(err) if err.to_string().contains("Locked") => last_lock_error = Some(err),
            Err(err) => panic!("reopen persistent follower: {err:?}"),
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!(
        "reopen persistent follower: lock never released: {:?}",
        last_lock_error.expect("at least one attempt")
    );
}

/// COUNTERFACTUAL: a single-attempt terminal mark produces exactly one
/// failed flush (1 attempt, 1 failure, 0 successes), not the 3/2/1 record.
#[cfg(unix)]
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

    let follower_tmp = tempfile::tempdir().expect("follower system-store directory");
    let follower_meta = follower_tmp.path().join("shamir_db_meta.redb");
    let follower = persistent_follower(follower_meta.clone()).await;
    create_profile_and_subscription(&follower).await;
    let sup =
        SubscriptionSupervisor::new(follower.clone(), factory, "follower-1").with_max_iterations(1);

    // loop#1: applies v1, exits, self-deregisters, lease invalidated.
    sup.reconcile().await;
    sup.wait_for_subscription("sub_a").await;
    wait_until_bookmark_covers(&follower, "main", version).await;

    // Reconcile#2: the start_race_hook arms the REAL catalogue-flush seam
    // when it fires (after the barrier flush, before spawn) and snapshots
    // the seam counters IMMEDIATELY (the diff baseline — the barrier flush
    // happened before arming and must not pollute). The RAII fault guard is
    // parked in a captured `Mutex<Option<_>>` and dropped only AFTER the
    // assertions, so the seam stays armed while loop#2 runs.
    let fault_slot: Arc<Mutex<Option<SeedCatalogueFlushFaultForTest>>> = Arc::new(Mutex::new(None));
    let baseline: Arc<Mutex<(usize, usize)>> = Arc::new(Mutex::new((0, 0)));
    let hook_follower = follower.clone();
    let hook_slot = fault_slot.clone();
    let hook_baseline = baseline.clone();
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
        let baseline = hook_baseline.clone();
        Box::pin(async move {
            let guard = SeedCatalogueFlushFaultForTest::arm_fail_first(&follower, 2);
            *baseline.lock().expect("counter baseline") = (guard.attempts(), guard.failures());
            *slot.lock().expect("fault slot") = Some(guard);
        })
    }));

    // Barrier passes (the hook arms the seam AFTER it); loop#2 spawns and
    // trips the identity fence; the wrapper's marking fails twice at its
    // flush seam and succeeds on attempt 3.
    sup2.reconcile().await;
    sup2.wait_for_subscription("sub_a").await; // joins loop#2 + its marking

    // Round-7 item 3: the mark must have returned only after the SUCCESSFUL
    // flush — the seam recorded EXACTLY 3 flush attempts with 2 injected
    // failures during the marking window (diffed against the post-arming
    // baseline so barrier flushes cannot pollute). Read BEFORE disarming.
    let (attempts, failures) = {
        let slot = fault_slot.lock().expect("fault slot");
        let (base_a, base_f) = *baseline.lock().expect("counter baseline");
        let guard = slot.as_ref().expect("seam still armed");
        (guard.attempts() - base_a, guard.failures() - base_f)
    };
    assert_eq!(
        attempts, 3,
        "the marking must have retried to a SUCCESSFUL flush: 2 failed attempts + 1 success"
    );
    assert_eq!(
        failures, 2,
        "exactly the 2 injected failures must have been consumed"
    );
    {
        let slot = fault_slot.lock().expect("fault slot");
        let guard = slot.as_ref().expect("seam still armed");
        assert_eq!(guard.successes(), 1, "the third marking flush must succeed");
    }

    // Release the seam only after the assertions (drop the RAII fault guard).
    *fault_slot.lock().expect("fault slot") = None;

    // Documented stop path before the reopen: `stop_all` joins any live
    // handles (the loops already exited via max_iterations). The
    // `with_start_race_hook` closure's `hook_follower` clone drops with
    // `sup2`, i.e. after `stop_all`.
    sup2.stop_all().await;
    sup.stop_all().await;
    drop(sup2);
    drop(sup);
    drop(fault_slot);
    drop(baseline);

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

    // Deterministic writer stop BEFORE the drop: the system repo's changefeed
    // journal writer owns the `__changelog__` Fjall partition store until its
    // channel closes, and that close is gated on background ticks that do not
    // reliably run before the reopen under `start_paused`. The module contract
    // requires stop/drain before re-opening the same store (changefeed.rs),
    // so use the graceful API instead of waiting. The lock-only retry helper
    // below stays as a safety net; it must not be what makes this pass.
    follower
        .stop_system_journal_writer_for_test()
        .await
        .expect("stop follower system journal writer");

    drop(follower);
    let reopened = Arc::new(reopen_follower_with_lock_retry(follower_meta).await);
    assert_eq!(
        sub_state(&reopened, "sub_a").await.as_deref(),
        Some("resync_required"),
        "the terminal state must survive reopening the system store"
    );
    assert!(
        reopened
            .has_resync_required_subscription_covering("app", "main")
            .await
            .unwrap(),
        "the reopened quarantine must cover app/main"
    );
}
