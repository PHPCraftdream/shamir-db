//! S5c-X PR8 chunk 4 (`durability.replication_serve = completed_reseed`:
//! SERVER FENCES) — the identity/epoch fence must QUARANTINE the follower.
//!
//! Scenarios:
//!   1. a history-epoch mismatch on Pull errors BEFORE any payload decode or
//!      apply (bookmark stays 0, journal stays empty, the pull count proves
//!      the fence — not the apply path — consumed the response);
//!   2. a legacy peer advertising `repo_lineage_id: None` / `history_epoch:
//!      None` is refused (`MissingIdentity`) and NO identity is adopted;
//!   3. through the supervisor, the fence trip persists
//!      `state = "resync_required"` on the subscription row (quarantine);
//!   4. NEGATIVE CONTROL: with `SKIP_FENCE_QUARANTINE_FOR_TEST` set the same
//!      epoch mismatch does NOT mark `resync_required` — the quarantine is
//!      load-bearing, not decorative.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use shamir_db::types::value::QueryValue;
use shamir_db::{DbError, SeedRefusal, ShamirDb};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{
    repl_scope, replication_profile, replication_status, subscription,
};
use shamir_query_builder::Query;
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_query_types::batch::BatchError;
use shamir_query_types::wire::repl::{ReplRepoInfo, ReplResponse};
use tokio_util::sync::CancellationToken;

use super::identity_fence_tests::{
    assert_identity_mismatch, build_memory, repo, write_rows, StaticHelloSource,
};
use crate::replication::error::ReplError;
use crate::replication::follower_loop::{run_follower_loop, FollowerLoopConfig};
use crate::replication::identity_fence::{checks, recorded};
use crate::replication::in_process::InProcessReplSource;
use crate::replication::source::ReplSource;
use crate::replication::supervisor::{ReplSourceFactory, SubscriptionSupervisor};

/// Serializes tests that touch the `SKIP_FENCE_QUARANTINE_FOR_TEST` seam:
/// in-process test runners may execute tests concurrently, and the negative
/// control must not leak into the quarantine test (nextest isolates processes
/// anyway; this keeps `cargo test` deterministic too).
static QUARANTINE_SEAM_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A [`ReplSource`] serving one FIXED Hello and one FIXED Pull reply.
struct FixedHelloPullSource {
    hello: ReplResponse,
    pull: ReplResponse,
    pulls: AtomicUsize,
}

#[async_trait]
impl ReplSource for FixedHelloPullSource {
    async fn hello(&self, _node_id: &str) -> Result<ReplResponse, ReplError> {
        Ok(self.hello.clone())
    }

    async fn pull(
        &self,
        _db: &str,
        _repo: &str,
        _from_version: u64,
        _limit: u32,
        _wait_ms: Option<u32>,
    ) -> Result<ReplResponse, ReplError> {
        self.pulls.fetch_add(1, Ordering::AcqRel);
        Ok(self.pull.clone())
    }
}

/// The supervisor-level scenario: a subscription whose recorded identity is
/// epoch 1 while the leader's live Pull advertises epoch 2. Returns the final
/// subscription state from the catalogue.
///
/// With `skip_quarantine = false` the fence trip must persist
/// `resync_required`; with the negative-control seam set the marking must be
/// skipped (the generic terminal path still records the typed refusal code).
async fn epoch_mismatch_subscription_state(skip_quarantine: bool) -> (Option<String>, usize) {
    let _guard = QUARANTINE_SEAM_LOCK.lock().await;
    checks::SKIP_FENCE_QUARANTINE_FOR_TEST.store(skip_quarantine, Ordering::SeqCst);

    let leader = build_memory().await;
    write_rows(&leader, 0, 2).await;
    let inner = InProcessReplSource::new(leader.clone());
    // Recorded identity: first contact at history epoch 1.
    repo(&leader).bump_history_epoch().await.unwrap();
    let hello = inner.hello("probe").await.unwrap();
    // The leader then serves Pulls as incarnation epoch 2 — a fence trip.
    repo(&leader).bump_history_epoch().await.unwrap();
    let source = Arc::new(StaticHelloSource {
        hello,
        inner: Arc::new(InProcessReplSource::new(leader)),
        pulls: AtomicUsize::new(0),
    });

    let follower = Arc::new(build_memory().await);
    let mut batch = Batch::new();
    batch.op(
        "profile",
        replication_profile("prof_reseed").stream(
            repl_scope("app").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    batch.op(
        "sub",
        subscription("sub_reseed", "tcp://leader:9000", "pub", "prof_reseed"),
    );
    let created = follower.execute("app", &batch.build()).await.unwrap();
    assert!(!created.results.contains_key("__error"), "{created:?}");

    let shared = source.clone();
    let factory: ReplSourceFactory = Arc::new(move |_| shared.clone());
    let supervisor = SubscriptionSupervisor::new(follower.clone(), factory, "probe")
        .with_poll_wait_ms(0)
        .with_max_iterations(1);
    supervisor.reconcile().await;
    // Join the loop task: the quarantine marking happens inside it, so once
    // the task is joined the catalogue state is final (bounded fallback wait
    // below for robustness).
    supervisor.wait_for_subscription("sub_reseed").await;
    let mut state = None;
    for _ in 0..300 {
        state = subscription_state(&follower, "sub_reseed").await;
        if state.as_deref() != Some("active") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    supervisor.stop_all().await;
    let pulls = source.pulls.load(Ordering::Acquire);
    checks::SKIP_FENCE_QUARANTINE_FOR_TEST.store(false, Ordering::SeqCst);
    (state, pulls)
}

/// Read the subscription's `state` through the ReplicationStatus admin op.
async fn subscription_state(follower: &ShamirDb, name: &str) -> Option<String> {
    let mut batch = Batch::new();
    batch.op("status", replication_status());
    let status = follower.execute("app", &batch.build()).await.ok()?;
    let value = status.results["status"].records.first()?.as_value();
    let QueryValue::List(entries) = &value["subscriptions"] else {
        return None;
    };
    entries
        .iter()
        .find(|row| row["name"].as_str() == Some(name))
        .and_then(|row| row["state"].as_str().map(String::from))
}

// ---------------------------------------------------------------------------
// 1 — epoch mismatch on Pull rejects BEFORE any payload decode/apply
// ---------------------------------------------------------------------------

#[tokio::test]
async fn epoch_mismatch_rejects_pull_before_apply() {
    let leader = build_memory().await;
    // Real (decodable, applicable) events in the reply: if the fence were
    // skipped the loop would decode and apply them — bookmark 2, non-empty
    // journal. The assertions below prove that never happened.
    write_rows(&leader, 0, 2).await;
    let follower = build_memory().await;
    let inner = InProcessReplSource::new(leader.clone());
    // Recorded identity: first contact at history epoch 1.
    repo(&leader).bump_history_epoch().await.unwrap();
    let hello = inner.hello("probe").await.unwrap();
    checks::fence_hello_recorded(&follower, "app", "main", &hello, None)
        .await
        .unwrap();
    // Live pull now advertises history epoch 2.
    repo(&leader).bump_history_epoch().await.unwrap();
    let pull = inner.pull("app", "main", 1, 1000, Some(0)).await.unwrap();
    let source = Arc::new(FixedHelloPullSource {
        hello,
        pull,
        pulls: AtomicUsize::new(0),
    });

    let before = repo(&follower).current_commit_version().await.unwrap();
    let error = run_follower_loop(
        Arc::new(follower.clone()),
        source.clone(),
        FollowerLoopConfig::new("probe", "app", "main")
            .with_poll_wait_ms(0)
            .with_max_iterations(1),
        CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_identity_mismatch(&error, SeedRefusal::WrongHistoryEpoch);
    let ReplError::IdentityMismatch {
        code,
        recorded_epoch,
        observed_epoch,
        ..
    } = &error
    else {
        unreachable!("asserted above");
    };
    assert_eq!(*code, SeedRefusal::WrongHistoryEpoch);
    assert_eq!(*recorded_epoch, 1);
    assert_eq!(*observed_epoch, 2);
    assert_eq!(source.pulls.load(Ordering::Acquire), 1);
    // No bookmark advance, no applied rows.
    assert_eq!(repo(&follower).replication_bookmark().await.unwrap(), 0);
    assert_eq!(
        repo(&follower).current_commit_version().await.unwrap(),
        before
    );
    assert!(repo(&follower)
        .read_changelog_bounded(0, 1000)
        .await
        .unwrap()
        .read
        .events
        .is_empty());
    // The recorded first-contact identity is unchanged; the offending
    // observation was persisted as metadata only.
    let info = repo(&follower).tx_info_store().await.unwrap();
    let record = recorded::load(&info).await.unwrap().unwrap();
    assert_eq!(record.history_epoch, 1);
    assert_eq!(
        record.observed_identity.map(|obs| obs.history_epoch),
        Some(2)
    );
}

// ---------------------------------------------------------------------------
// 2 — legacy peer without any identity is refused; nothing is adopted
// ---------------------------------------------------------------------------

#[tokio::test]
async fn legacy_peer_without_identity_is_refused() {
    let follower = build_memory().await;
    let hello = ReplResponse::Hello {
        leader_epoch: 1,
        repos: vec![ReplRepoInfo {
            db: "app".into(),
            repo: "main".into(),
            current_version: 0,
            journal_floor: 0,
            repo_lineage_id: None,
            history_epoch: None,
            gap_first: None,
            gap_through: None,
            schema_digest: None,
            settled_version: None,
            publications: Vec::new(),
        }],
    };
    assert!(matches!(
        checks::fence_hello_recorded(&follower, "app", "main", &hello, None).await,
        Err(ReplError::Seed(SeedRefusal::MissingIdentity))
    ));
    let pull = ReplResponse::Pull {
        leader_epoch: 1,
        events: Vec::new(),
        gap_at: None,
        current_version: 0,
        repo_lineage_id: None,
        history_epoch: None,
        gap_first: None,
        gap_through: None,
        schema_digest: None,
    };
    let info = repo(&follower).tx_info_store().await.unwrap();
    assert!(matches!(
        checks::fence_pull_identity(&info, &pull, None).await,
        Err(ReplError::Seed(SeedRefusal::MissingIdentity))
    ));
    // Fail-closed: a None identity never adopts a record.
    assert_eq!(recorded::load(&info).await.unwrap(), None);
}

// ---------------------------------------------------------------------------
// 3 + 4 — supervisor-level quarantine and its negative control
// ---------------------------------------------------------------------------

#[tokio::test]
async fn epoch_mismatch_marks_subscription_resync_required() {
    let (state, pulls) = epoch_mismatch_subscription_state(false).await;
    assert_eq!(pulls, 1, "the fence must have consumed exactly one pull");
    assert_eq!(
        state.as_deref(),
        Some("resync_required"),
        "an identity/epoch fence trip must quarantine the subscription"
    );
}

#[tokio::test]
async fn negative_control_without_quarantine_state_is_not_resync_required() {
    let (state, pulls) = epoch_mismatch_subscription_state(true).await;
    assert_eq!(pulls, 1, "the loop must still have hit the fence");
    assert_ne!(
        state.as_deref(),
        Some("resync_required"),
        "with the seam set the quarantine must NOT fire"
    );
    // The loop still terminates; the generic terminal path persists the typed
    // refusal code instead of the quarantine state.
    assert_eq!(
        state.as_deref(),
        Some(SeedRefusal::WrongHistoryEpoch.code())
    );
}

// ---------------------------------------------------------------------------
// 5 — the persisted quarantine fences ordinary READS on the follower
// ---------------------------------------------------------------------------

/// The real supervisor fence path (epoch mismatch → quarantine marking), but
/// keeps the follower's `ShamirDb` handle so the fenced read surface can be
/// exercised. Mirrors [`epoch_mismatch_subscription_state`] with
/// `skip_quarantine = false` and owns the same seam lock.
async fn quarantined_follower() -> Arc<ShamirDb> {
    let _guard = QUARANTINE_SEAM_LOCK.lock().await;
    checks::SKIP_FENCE_QUARANTINE_FOR_TEST.store(false, Ordering::SeqCst);

    let leader = build_memory().await;
    write_rows(&leader, 0, 2).await;
    let inner = InProcessReplSource::new(leader.clone());
    repo(&leader).bump_history_epoch().await.unwrap();
    let hello = inner.hello("probe").await.unwrap();
    repo(&leader).bump_history_epoch().await.unwrap();
    let source = Arc::new(StaticHelloSource {
        hello,
        inner: Arc::new(InProcessReplSource::new(leader)),
        pulls: AtomicUsize::new(0),
    });

    let follower = Arc::new(build_memory().await);
    let mut batch = Batch::new();
    batch.op(
        "profile",
        replication_profile("prof_reseed").stream(
            repl_scope("app").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    batch.op(
        "sub",
        subscription("sub_reseed", "tcp://leader:9000", "pub", "prof_reseed"),
    );
    let created = follower.execute("app", &batch.build()).await.unwrap();
    assert!(!created.results.contains_key("__error"), "{created:?}");

    let shared = source.clone();
    let factory: ReplSourceFactory = Arc::new(move |_| shared.clone());
    let supervisor = SubscriptionSupervisor::new(follower.clone(), factory, "probe")
        .with_poll_wait_ms(0)
        .with_max_iterations(1);
    supervisor.reconcile().await;
    // Join the loop task: once it is joined the quarantine marking (inside
    // the task) is durably in the catalogue — no sleeps needed.
    supervisor.wait_for_subscription("sub_reseed").await;
    supervisor.stop_all().await;
    checks::SKIP_FENCE_QUARANTINE_FOR_TEST.store(false, Ordering::SeqCst);
    follower
}
/// After the REAL fence trip persisted `resync_required`:
///   (a) ordinary reads through the same handle fail with the typed
///       `BatchError` code `resync_required`;
///   (b) `read_changelog_bounded` fails with `DbError::ResyncRequired`;
///   (c) the admin status read still succeeds and still reports the
///       quarantined state.
#[tokio::test]
async fn quarantined_subscription_fences_reads_but_not_status() {
    let follower = quarantined_follower().await;

    // (a) ordinary SELECT of the destination repo is refused.
    let mut read = Batch::new();
    read.query("r", Query::with_repo("main", "items"));
    let err = follower.execute("app", &read.build()).await.unwrap_err();
    assert_eq!(err.code(), Some("resync_required"));
    assert!(matches!(err, BatchError::ResyncRequired { .. }));

    // (b) the changelog pull path refuses too.
    let err = follower
        .read_changelog_bounded("app", "main", 1, 10)
        .await
        .unwrap_err();
    assert!(matches!(err, DbError::ResyncRequired { .. }));

    // (c) admin status stays readable and shows the quarantine state.
    assert_eq!(
        subscription_state(&follower, "sub_reseed").await.as_deref(),
        Some("resync_required"),
    );
}
