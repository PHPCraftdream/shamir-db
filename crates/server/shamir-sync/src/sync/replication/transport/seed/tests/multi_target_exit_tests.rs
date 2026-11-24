//! Round-6 item 4 — multi-target exit: REFERENCE-COUNTED teardown. All
//! target loops of one start share ONE lease; only the LAST exiting loop —
//! the only one guaranteed that no sibling still holds an apply read guard
//! (all loops have returned) — invalidates the lease and removes the handle.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::poll;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{repl_scope, replication_profile, subscription};
use shamir_query_builder::doc;
use shamir_query_builder::write::Insert;
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_query_types::wire::repl::ReplResponse;
use tokio::sync::Notify;

use crate::replication::error::ReplError;
use crate::replication::follower_loop::ApplyParkHook;
use crate::replication::in_process::InProcessReplSource;
use crate::replication::source::ReplSource;
use crate::replication::supervisor::{ReplSourceFactory, Subscription, SubscriptionSupervisor};

use super::lease_gate_tests::{build_db, write_rows};

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
        .execute(
            "app",
            &b.to_request_via_msgpack().expect("batch request encoding"),
        )
        .await
        .expect("create two-target subscription");
}

/// Write `n` rows into `app/other/items` on the leader (the `other` loop's
/// first pull must carry events, or it never reaches the apply park).
async fn write_other_rows(leader: &ShamirDb, base: usize, n: usize) {
    let owner =
        shamir_db::access::Actor::User(shamir_db::access::principal64_from_username("alice"));
    for i in base..(base + n) {
        let mut batch = Batch::named("ins");
        batch.id("ins");
        batch.transactional();
        batch.insert(
            "i",
            Insert::with_repo("other", "items").rows([doc! {
                "id" => format!("o{i}"),
                "v" => i as i64,
            }]),
        );
        let resp = leader
            .execute_as(owner.clone(), "app", &batch.build())
            .await
            .expect("fixture write");
        assert!(
            !resp.results.contains_key("__error"),
            "write failed: {resp:?}"
        );
    }
}

/// Source wrapper dispatching per destination (ONE instance is shared by
/// BOTH target loops — the supervisor clones one `Arc` into every spawn):
/// `app/main` pulls answer honestly ONCE, then serve a REGRESSED leader
/// epoch after an explicit trigger; `app/other` applies its batch and parks
/// inside apply, then returns a stale epoch after release so teardown finishes.
struct StaleMainHonestOtherSource {
    inner: InProcessReplSource,
    main_pulls: AtomicUsize,
    other_pulls: AtomicUsize,
    main_second_pull: Arc<Notify>,
}
#[async_trait]
impl ReplSource for StaleMainHonestOtherSource {
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
        if repo == "main" && self.main_pulls.fetch_add(1, Ordering::SeqCst) != 0 {
            // Second contact: a regressed epoch → StaleLeaderEpoch in the
            // loop (leader epochs start at 1).
            self.main_second_pull.notified().await;
            let mut regressed = self
                .inner
                .pull(db, repo, from_version, limit, Some(0))
                .await?;
            if let ReplResponse::Pull { leader_epoch, .. } = &mut regressed {
                *leader_epoch = 0;
            }
            return Ok(regressed);
        }
        if repo == "other" && self.other_pulls.fetch_add(1, Ordering::SeqCst) != 0 {
            let mut stale = self
                .inner
                .pull(db, repo, from_version, limit, Some(0))
                .await?;
            if let ReplResponse::Pull { leader_epoch, .. } = &mut stale {
                *leader_epoch = 0;
            }
            return Ok(stale);
        }
        self.inner
            .pull(db, repo, from_version, limit, wait_ms)
            .await
    }
}

/// Scoping the silent `main` exit proves the still-live sibling preserves the lease.
struct ReleasePark(Arc<Notify>);
impl Drop for ReleasePark {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

#[tokio::test]
async fn multi_target_exit_refcount_last_exit_removes() {
    let leader = build_db().await;
    // Leader rows BEFORE the start in BOTH repos: each loop's first contact
    // carries events and applies immediately (no long-poll — determinism).
    write_rows(&leader, 0, 3).await;
    write_other_rows(&leader, 0, 2).await;

    let follower = build_db().await;
    create_two_target_subscription(&follower).await;

    let main_second_pull = Arc::new(Notify::new());

    let factory: ReplSourceFactory = {
        let leader = leader.clone();
        let main_second_pull = Arc::clone(&main_second_pull);
        Arc::new(move |_sub: &Subscription| {
            Arc::new(StaleMainHonestOtherSource {
                inner: InProcessReplSource::new((*leader).clone()),
                main_pulls: AtomicUsize::new(0),
                other_pulls: AtomicUsize::new(0),
                main_second_pull: Arc::clone(&main_second_pull),
            }) as Arc<dyn ReplSource>
        })
    };
    // Run-forever supervisor; short long-poll budget so the final teardown
    // joins promptly.
    let arrived = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let exited = Arc::new(Notify::new());
    let _cleanup = ReleasePark(Arc::clone(&release));
    let sup = SubscriptionSupervisor::new(follower.clone(), factory, "follower-1")
        .with_poll_wait_ms(100)
        .with_apply_park(Arc::new(ApplyParkHook {
            repo: Some("other".into()),
            after_apply: false,
            arrived: Arc::clone(&arrived),
            release: Arc::clone(&release),
            exited: Arc::clone(&exited),
        }));
    let teardown = sup.teardown_signal();
    let teardown_done = teardown.notified();
    tokio::pin!(teardown_done);
    sup.reconcile().await; // spawns TWO loops under ONE shared lease
    assert_eq!(sup.active_count(), 1, "one handle, two target loops");

    // `main` applies its honest batch, then its second pull is explicitly
    // triggered to return a regressed leader epoch and exit.
    arrived.notified().await;
    main_second_pull.notify_one();
    exited.notified().await;
    assert_eq!(sup.teardown_count(), 0);
    assert!(matches!(
        poll!(&mut teardown_done),
        std::task::Poll::Pending
    ));
    assert_eq!(
        follower
            .subscription_lease("sub_a")
            .map(|l| l.valid.load(Ordering::Acquire)),
        Some(true),
        "lease must remain valid while the other apply guard is held"
    );
    let lease = follower.subscription_lease("sub_a").unwrap();
    {
        let mut writer = Box::pin(lease.apply_gate.write());
        assert!(matches!(poll!(writer.as_mut()), std::task::Poll::Pending));
    }

    // MID-TEST refcount property: `main` has exited, but the parked sibling
    // holds the refcount — the lease is STILL valid and the handle STILL
    // registered (the LAST exit has not happened).
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

    release.notify_one();
    teardown_done.await;
    assert_eq!(sup.teardown_count(), 1);
    assert!(!sup.is_running("sub_a"));
    assert_eq!(
        follower
            .subscription_lease("sub_a")
            .map(|l| l.valid.load(Ordering::Acquire)),
        None
    );
}

/// A source whose Hello or Pull never answers: the cancellation checkpoints of
/// the loop must not wait for a silent upstream (the production request
/// timeout is 35 s). `entered` is signalled once the silent call is awaited.
struct SilentSource {
    inner: InProcessReplSource,
    silent_hello: bool,
    entered: Arc<Notify>,
}
#[async_trait]
impl ReplSource for SilentSource {
    async fn hello(&self, node_id: &str) -> Result<ReplResponse, ReplError> {
        if self.silent_hello {
            self.entered.notify_one();
            return std::future::pending().await;
        }
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
        if !self.silent_hello {
            self.entered.notify_one();
            return std::future::pending().await;
        }
        self.inner
            .pull(db, repo, from_version, limit, wait_ms)
            .await
    }
}

fn honest_factory(leader: &Arc<ShamirDb>) -> ReplSourceFactory {
    let leader = leader.clone();
    Arc::new(move |_sub: &Subscription| {
        Arc::new(InProcessReplSource::new((*leader).clone())) as Arc<dyn ReplSource>
    })
}

/// The row is still ACTIVE in the catalogue (so a pass that ignored the stop
/// would start it again).
async fn assert_row_active(sup: &SubscriptionSupervisor) {
    let rows = sup.read_subscriptions().await.expect("catalogue read");
    assert!(
        rows.iter()
            .any(|row| row.name == "sub_a" && row.is_active()),
        "sub_a must still be an active row: {rows:?}"
    );
}

/// `begin_stop` cancels every running follower WITHOUT the lifecycle lock and
/// without awaiting anything, and a later pass starts nothing although the
/// subscription row is still active.
///
/// Mutants: a `begin_stop` that cancels nothing leaves the loops running (the
/// bounded teardown wait fails); a reconcile that ignores the stop token
/// restarts the row.
#[tokio::test]
async fn begin_stop_cancels_followers_without_the_lock_and_fences_later_starts() {
    let leader = build_db().await;
    write_rows(&leader, 0, 3).await;
    write_other_rows(&leader, 0, 2).await;
    let follower = build_db().await;
    create_two_target_subscription(&follower).await;
    let sup = SubscriptionSupervisor::new(follower.clone(), honest_factory(&leader), "follower-1")
        .with_poll_wait_ms(100);
    let teardown = sup.teardown_signal();
    sup.reconcile().await;
    assert!(sup.is_running("sub_a"), "the follower must be running");

    // No `stop_all`, no lifecycle lock: only the stop token signals the loops.
    sup.begin_stop();
    tokio::time::timeout(Duration::from_secs(60), teardown.notified())
        .await
        .expect("every follower loop must exit after begin_stop alone");
    assert_eq!(sup.teardown_count(), 1);
    assert!(!sup.is_running("sub_a"));

    assert_row_active(&sup).await;
    sup.reconcile().await;
    assert!(
        !sup.is_running("sub_a"),
        "a stopping supervisor must start no follower"
    );
    assert_eq!(sup.active_count(), 0);
}

/// A follower that registers AFTER `begin_stop` was called is born cancelled:
/// an admitted reconcile is parked before its spawn (by the start race hook,
/// which runs under the lifecycle lock), `begin_stop` runs meanwhile, and the
/// released start must still end up with no running follower, without
/// `stop_all`.
///
/// Mutant: a start that builds its token without the stop token as parent
/// leaves the late follower running (the bounded teardown wait fails).
#[tokio::test]
async fn a_follower_started_after_begin_stop_is_born_cancelled() {
    let leader = build_db().await;
    write_rows(&leader, 0, 3).await;
    write_other_rows(&leader, 0, 2).await;
    let follower = build_db().await;
    create_two_target_subscription(&follower).await;
    let entered = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(Notify::new());
    let (entered_h, release_h) = (Arc::clone(&entered), Arc::clone(&release));
    let sup = SubscriptionSupervisor::new(follower.clone(), honest_factory(&leader), "follower-1")
        .with_poll_wait_ms(100)
        .with_start_race_hook(Arc::new(move || {
            let entered = Arc::clone(&entered_h);
            let release = Arc::clone(&release_h);
            Box::pin(async move {
                entered.fetch_add(1, Ordering::SeqCst);
                release.notified().await;
            })
        }));
    let teardown = sup.teardown_signal();

    let mut reconcile = Box::pin(sup.reconcile());
    // `poll!` expands to code containing `.await`: poll the future here until
    // the hook is entered (the start is admitted and parked before its spawn).
    for _ in 0..10_000 {
        if entered.load(Ordering::SeqCst) > 0 {
            break;
        }
        let _ = poll!(reconcile.as_mut());
        tokio::task::yield_now().await;
    }
    assert_eq!(
        entered.load(Ordering::SeqCst),
        1,
        "the start must be parked"
    );
    assert!(!sup.is_running("sub_a"), "nothing is registered yet");

    sup.begin_stop();
    release.notify_one();
    reconcile.await;

    tokio::time::timeout(Duration::from_secs(60), teardown.notified())
        .await
        .expect("the late follower must exit without stop_all");
    assert_eq!(sup.teardown_count(), 1);
    assert!(!sup.is_running("sub_a"));
}

/// A follower that waits for a silent upstream (Hello or Pull never answers)
/// exits on `begin_stop` at once instead of after the request timeout.
///
/// Mutant: a Hello or Pull awaited without a cancellation select leaves the
/// loop waiting (the bounded teardown wait fails).
#[tokio::test]
async fn begin_stop_does_not_wait_for_a_silent_upstream() {
    for silent_hello in [true, false] {
        let leader = build_db().await;
        write_rows(&leader, 0, 3).await;
        write_other_rows(&leader, 0, 2).await;
        let follower = build_db().await;
        create_two_target_subscription(&follower).await;
        let entered = Arc::new(Notify::new());
        let factory: ReplSourceFactory = {
            let leader = leader.clone();
            let entered = Arc::clone(&entered);
            Arc::new(move |_sub: &Subscription| {
                Arc::new(SilentSource {
                    inner: InProcessReplSource::new((*leader).clone()),
                    silent_hello,
                    entered: Arc::clone(&entered),
                }) as Arc<dyn ReplSource>
            })
        };
        let sup = SubscriptionSupervisor::new(follower.clone(), factory, "follower-1");
        let teardown = sup.teardown_signal();
        sup.reconcile().await;
        tokio::time::timeout(Duration::from_secs(60), entered.notified())
            .await
            .expect("a loop must reach the silent call");

        sup.begin_stop();

        tokio::time::timeout(Duration::from_secs(60), teardown.notified())
            .await
            .unwrap_or_else(|_| {
                panic!("silent_hello={silent_hello}: the loops must exit on begin_stop")
            });
        assert!(!sup.is_running("sub_a"));
    }
}
