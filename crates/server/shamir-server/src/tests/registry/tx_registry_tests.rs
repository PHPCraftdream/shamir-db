use std::sync::Arc;
use std::time::{Duration, Instant};

use shamir_db::engine::repo::{BoxRepo, RepoInstance};
use shamir_db::engine::table::TableConfig;
use shamir_db::storage::storage_in_memory::InMemoryRepo;
use shamir_types::types::value::InnerValue;

use crate::tx_registry::{
    no_abort_release, spawn_reaper_task, InteractiveTx, TxRegistry, TxRegistryError,
};

const SID_A: [u8; 32] = [0xAA; 32];
const SID_B: [u8; 32] = [0xBB; 32];

/// Build a real `(handle, InteractiveTx)` from a standalone gate — the
/// registry stores genuine `TxContext` + `SnapshotGuard` values.
async fn make_tx(sid: [u8; 32], max_lifetime: Duration, seed: u64) -> (u64, InteractiveTx) {
    // `seed` is the gate's first `fresh_tx_id()` — pass distinct seeds
    // when a test needs distinct handles (each call builds a fresh gate).
    let gate = shamir_tx::RepoTxGate::new(0, seed);
    let guard = gate.open_snapshot().await;
    let tx_id = gate.fresh_tx_id();
    let tx = shamir_tx::TxContext::new(
        tx_id,
        0,
        guard.version(),
        shamir_tx::IsolationLevel::Snapshot,
    );
    let it = InteractiveTx::new(
        tx,
        guard,
        sid,
        [0u8; 16],
        "db".to_string(),
        "repo".to_string(),
        max_lifetime,
        no_abort_release(),
    );
    (tx_id.0, it)
}

#[tokio::test(start_paused = true)]
async fn remaining_lifetime_tracks_absolute_deadline_without_activity_extension() {
    let (_, it) = make_tx(SID_A, Duration::from_secs(60), 1).await;
    assert_eq!(it.remaining_lifetime(), Duration::from_secs(60));
    tokio::time::advance(Duration::from_secs(17)).await;
    assert_eq!(it.remaining_lifetime(), Duration::from_secs(43));
    it.bump_activity();
    assert_eq!(it.remaining_lifetime(), Duration::from_secs(43));
    tokio::time::advance(Duration::from_secs(43)).await;
    assert_eq!(it.remaining_lifetime(), Duration::ZERO);
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(it.remaining_lifetime(), Duration::ZERO);
}

#[tokio::test(start_paused = true)]
async fn remaining_lifetime_zero_at_creation_and_after_expiry() {
    let (_, it) = make_tx(SID_A, Duration::ZERO, 1).await;
    assert_eq!(it.remaining_lifetime(), Duration::ZERO);
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(it.remaining_lifetime(), Duration::ZERO);
}

#[tokio::test]
async fn remove_for_session_removes_only_that_sessions_tx_and_frees_slot() {
    let reg = TxRegistry::new();
    let (handle_a, tx_a) = make_tx(SID_A, Duration::from_secs(300), 1).await;
    let (handle_b, tx_b) = make_tx(SID_B, Duration::from_secs(300), 2).await;
    reg.register(handle_a, tx_a).unwrap();
    reg.register(handle_b, tx_b).unwrap();

    let removed = reg.remove_for_session(&SID_A).expect("owned tx removed");
    assert_eq!(removed.owner_sid(), &SID_A);
    assert!(reg.remove_for_session(&SID_A).is_none());
    assert!(matches!(
        reg.get_owned(handle_a, &SID_A),
        Err(TxRegistryError::TxNotFound)
    ));
    assert_eq!(reg.get_owned(handle_b, &SID_B).unwrap().owner_sid(), &SID_B);

    let (reused_handle, reused_tx) = make_tx(SID_A, Duration::from_secs(300), 3).await;
    reg.register(reused_handle, reused_tx)
        .expect("session slot reusable after teardown removal");
    assert_eq!(reg.len(), 2);
}

#[tokio::test]
async fn register_then_get_owned_roundtrip() {
    let reg = TxRegistry::new();
    let (handle, it) = make_tx(SID_A, Duration::from_secs(300), 1).await;
    reg.register(handle, it).unwrap();

    let got = reg.get_owned(handle, &SID_A).unwrap();
    assert_eq!(got.db(), "db");
    assert_eq!(got.repo(), "repo");
    assert_eq!(got.owner_user_id(), &[0u8; 16]);
    assert_eq!(reg.len(), 1);
}

#[tokio::test]
async fn one_tx_per_session_rejected() {
    let reg = TxRegistry::new();
    let (h1, it1) = make_tx(SID_A, Duration::from_secs(300), 1).await;
    let (h2, it2) = make_tx(SID_A, Duration::from_secs(300), 2).await;
    reg.register(h1, it1).unwrap();

    assert!(
        matches!(reg.register(h2, it2), Err(TxRegistryError::TxAlreadyOpen)),
        "second BEGIN on a session with an open tx must be rejected"
    );
    // The rejected tx left no trace.
    assert_eq!(reg.len(), 1);
    assert!(reg.get_owned(h2, &SID_A).is_err());
}

#[tokio::test]
async fn get_owned_foreign_session_rejected() {
    let reg = TxRegistry::new();
    let (handle, it) = make_tx(SID_A, Duration::from_secs(300), 1).await;
    reg.register(handle, it).unwrap();

    // Same handle, different session id → theft guard fires.
    assert!(matches!(
        reg.get_owned(handle, &SID_B),
        Err(TxRegistryError::TxOwnershipMismatch)
    ));
}

#[tokio::test]
async fn get_owned_unknown_handle() {
    let reg = TxRegistry::new();
    assert!(matches!(
        reg.get_owned(999, &SID_A),
        Err(TxRegistryError::TxNotFound)
    ));
}

#[tokio::test]
async fn remove_frees_session_slot() {
    let reg = TxRegistry::new();
    let (h1, it1) = make_tx(SID_A, Duration::from_secs(300), 1).await;
    reg.register(h1, it1).unwrap();

    let removed = reg.remove(h1).expect("handle present");
    assert_eq!(removed.owner_sid(), &SID_A);
    assert!(reg.is_empty());
    assert!(matches!(
        reg.get_owned(h1, &SID_A),
        Err(TxRegistryError::TxNotFound)
    ));

    // Session slot is freed → the session can open a new tx.
    let (h2, it2) = make_tx(SID_A, Duration::from_secs(300), 2).await;
    reg.register(h2, it2).unwrap();
    assert_eq!(reg.len(), 1);
}

#[tokio::test]
async fn take_ctx_closes_handle_on_commit() {
    let reg = TxRegistry::new();
    let (handle, it) = make_tx(SID_A, Duration::from_secs(300), 1).await;
    let arc = reg.register(handle, it).unwrap();

    // COMMIT/ROLLBACK semantics: take the TxContext out of the Arc.
    let taken = arc.ctx().lock().await.take();
    assert!(taken.is_some(), "first take yields the live TxContext");

    // A later call on the (now closed) handle sees None.
    let again = arc.ctx().lock().await.take();
    assert!(again.is_none(), "second take on a closed handle is None");
}

#[tokio::test]
async fn expired_by_absolute_deadline() {
    let reg = TxRegistry::new();
    // Zero lifetime → deadline == creation instant; monotonic time has
    // advanced by the assert, so `now >= deadline`.
    let (handle, it) = make_tx(SID_A, Duration::ZERO, 1).await;
    reg.register(handle, it).unwrap();

    let expired = reg.expired_handles(Instant::now(), Duration::from_secs(3600));
    assert_eq!(
        expired,
        vec![handle],
        "zero-lifetime tx is past its deadline"
    );
}

#[tokio::test]
async fn reaper_contract_past_deadline_tx_is_removed() {
    let reg = TxRegistry::new();
    // Zero lifetime -> deadline == creation instant; the registry's
    // is_expired check fires immediately. Same trick as
    // `expired_by_absolute_deadline`.
    let (handle, it) = make_tx(SID_A, Duration::ZERO, 1).await;
    reg.register(handle, it).unwrap();

    // The contract the reaper task runs each tick:
    let expired = reg.expired_handles(Instant::now(), Duration::from_secs(60));
    assert_eq!(expired, vec![handle], "past-deadline tx is listed by sweep");
    for h in expired {
        let arc = reg.remove(h);
        assert!(arc.is_some(), "remove yields the parked tx for RAII drop");
    }
    assert!(reg.is_empty(), "registry empty after sweep");
    assert!(
        matches!(
            reg.get_owned(handle, &SID_A),
            Err(TxRegistryError::TxNotFound)
        ),
        "lookup after reap returns TxNotFound"
    );
}

#[tokio::test(start_paused = true)]
async fn reaper_task_reaps_past_deadline_tx() {
    let reg = Arc::new(TxRegistry::new());
    let (handle, it) = make_tx(SID_A, Duration::ZERO, 1).await;
    reg.register(handle, it).unwrap();
    assert_eq!(reg.len(), 1);

    // Tight sweep, generous idle TTL -> only the absolute-deadline branch fires.
    let shutdown = tokio_util::sync::CancellationToken::new();
    let reaper = spawn_reaper_task(
        Arc::clone(&reg),
        Duration::from_secs(60),
        Duration::from_millis(50),
        shutdown.clone(),
    );
    // With paused time, advance past two intervals: the first tick is
    // dropped by spawn_reaper_task, the second is the real sweep.
    // Deterministic — no wall-clock dependence.
    // Advance in two steps to ensure the spawned task processes each tick:
    // 1) first interval tick (dropped by reaper on boot)
    tokio::time::advance(Duration::from_millis(50)).await;
    tokio::task::yield_now().await;
    // 2) second interval tick — the real sweep fires
    tokio::time::advance(Duration::from_millis(50)).await;
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
    assert!(reg.is_empty(), "reaper task drained the past-deadline tx");
    assert!(matches!(
        reg.get_owned(handle, &SID_A),
        Err(TxRegistryError::TxNotFound)
    ));

    // Clean drain -- mirror ServerHandle::shutdown so the test never leaks the task.
    shutdown.cancel();
    let _ = reaper.handle.await;
}

#[tokio::test]
async fn expired_by_idle_ttl() {
    let reg = TxRegistry::new();
    // Long absolute deadline, but a zero idle TTL → always idle-expired.
    let (handle, it) = make_tx(SID_A, Duration::from_secs(3600), 1).await;
    reg.register(handle, it).unwrap();

    let expired = reg.expired_handles(Instant::now(), Duration::ZERO);
    assert_eq!(expired, vec![handle], "zero idle-ttl reaps any inactive tx");

    // With a generous idle TTL and far deadline, nothing is expired.
    assert!(reg
        .expired_handles(Instant::now(), Duration::from_secs(3600))
        .is_empty());
}

/// Sweep workflow: `expired_handles` yields the reaped set; `remove`
/// drops the `InteractiveTx` (RAII = no storage side effect, design
/// §6.4) and frees the one-tx-per-session slot.
#[tokio::test]
async fn sweep_reaps_expired_handle_and_frees_session_slot() {
    let reg = TxRegistry::new();
    // absolute=0 → already expired
    let (handle, it) = make_tx(SID_A, Duration::ZERO, 1).await;
    reg.register(handle, it).unwrap();

    // Sweep step 1: collect expired handles.
    let expired = reg.expired_handles(Instant::now(), Duration::from_secs(3600));
    assert_eq!(
        expired,
        vec![handle],
        "sweep must surface the past-deadline handle"
    );

    // Sweep step 2: drop each — RAII rollback, no storage I/O.
    for h in expired {
        let arc = reg.remove(h).expect("sweep removes the entry");
        // Closing the handle on the sweep path mirrors
        // commit/rollback semantics.
        let _ = arc.ctx().lock().await.take();
    }

    assert!(reg.is_empty(), "sweep drained the open map");
    assert!(
        matches!(
            reg.get_owned(handle, &SID_A),
            Err(TxRegistryError::TxNotFound)
        ),
        "reaped handle is no longer addressable"
    );

    // Session slot is freed → the session can open a NEW tx (would have
    // hit TxAlreadyOpen if `remove` had skipped by_session cleanup).
    let (h2, it2) = make_tx(SID_A, Duration::from_secs(300), 2).await;
    reg.register(h2, it2)
        .expect("session slot freed after sweep");
    assert_eq!(reg.len(), 1);
}

/// `bump_activity` defers idle-deadline reaping but does NOT extend the
/// absolute deadline (the hard upper bound on how long a tx can pin GC).
#[tokio::test]
async fn bump_activity_defers_idle_reap() {
    let reg = TxRegistry::new();
    // Far absolute deadline.
    let (handle, it) = make_tx(SID_A, Duration::from_secs(3600), 1).await;
    let arc = reg.register(handle, it).unwrap();

    // Before bump: zero idle-ttl reaps any inactive tx.
    let expired_pre = reg.expired_handles(Instant::now(), Duration::ZERO);
    assert_eq!(
        expired_pre,
        vec![handle],
        "sanity: idle-reap fires at ZERO ttl"
    );

    // Bump activity — the idle clock restarts.
    arc.bump_activity();

    // With a generous idle ttl (1 hour), the just-bumped tx is NOT
    // idle-expired.
    assert!(
        reg.expired_handles(Instant::now(), Duration::from_secs(3600))
            .is_empty(),
        "bump_activity defers the idle-reap when the absolute deadline is far"
    );
}

/// Even after `bump_activity`, an absolute-deadline-past tx is reaped.
/// The absolute deadline is the hard upper bound (design doc §6.4).
#[tokio::test]
async fn absolute_deadline_overrides_bump_activity() {
    let reg = TxRegistry::new();
    // absolute=0 → already past
    let (handle, it) = make_tx(SID_A, Duration::ZERO, 1).await;
    let arc = reg.register(handle, it).unwrap();

    // Bump activity — but the absolute deadline is the hard cap.
    arc.bump_activity();

    // Even with a huge idle ttl, the past-absolute-deadline tx is
    // expired.
    let expired = reg.expired_handles(Instant::now(), Duration::from_secs(3600));
    assert_eq!(
        expired,
        vec![handle],
        "absolute deadline overrides bump_activity — it is the hard \
         upper bound on tx lifetime"
    );
}

#[tokio::test(start_paused = true)]
async fn long_lease_survives_repeated_reap_then_commits_real_write() {
    let repo = RepoInstance::new(
        "repo".into(),
        BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
        vec![TableConfig::new("items")],
    );
    let table = repo.get_table("items").await.unwrap();
    let (tx, snapshot) = repo
        .begin_tx(shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();
    let handle = tx.tx_id.0;
    let reg = TxRegistry::new();
    reg.register(
        handle,
        InteractiveTx::new(
            tx,
            snapshot,
            SID_A,
            [0; 16],
            "db".into(),
            "repo".into(),
            Duration::from_secs(300),
            no_abort_release(),
        ),
    )
    .unwrap();
    let (it, lease) = reg.acquire_owned(handle, &SID_A).unwrap();
    let rid = {
        let mut guard = it.ctx().lock().await;
        table
            .insert_tx(
                &InnerValue::Str("leased-write".into()),
                Some(guard.as_mut().unwrap()),
            )
            .await
            .unwrap()
    };
    for _ in 0..4 {
        tokio::time::advance(Duration::from_secs(31)).await;
        assert_eq!(reg.reap_expired(Duration::from_secs(30)), 0);
        assert_eq!(reg.len(), 1);
    }
    drop(lease);
    reg.get_owned(handle, &SID_A)
        .expect("commit still finds the handle");
    let parked = reg.remove(handle).unwrap();
    let tx = parked.ctx().lock().await.take().unwrap();
    let outcome = repo.commit_tx(tx).await.unwrap();
    assert!(outcome.commit_version > 0);
    assert_eq!(
        table.get(rid).await.unwrap(),
        InnerValue::Str("leased-write".into()),
    );
    assert!(reg.is_empty());
}

#[tokio::test(start_paused = true)]
async fn lease_release_restarts_full_idle_ttl_with_overlapping_leases() {
    let reg = TxRegistry::new();
    let ttl = Duration::from_secs(30);
    let (handle, it) = make_tx(SID_A, Duration::from_secs(300), 1).await;
    reg.register(handle, it).unwrap();
    tokio::time::advance(Duration::from_secs(20)).await;
    let (it, first) = reg.acquire_owned(handle, &SID_A).unwrap();
    assert!(!it.is_expired(tokio::time::Instant::now().into_std(), ttl));
    let (_, second) = reg.acquire_owned(handle, &SID_A).unwrap();
    tokio::time::advance(Duration::from_secs(40)).await;
    drop(first);
    tokio::time::advance(Duration::from_secs(31)).await;
    assert_eq!(
        reg.reap_expired(ttl),
        0,
        "second lease still protects the entry",
    );
    drop(second);
    tokio::time::advance(Duration::from_secs(29)).await;
    assert_eq!(reg.reap_expired(ttl), 0);
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(reg.reap_expired(ttl), 1);
    assert!(reg.is_empty());
    let (next, it) = make_tx(SID_A, Duration::from_secs(300), 2).await;
    reg.register(next, it)
        .expect("reaping frees the session slot");
}

#[tokio::test(start_paused = true)]
async fn abort_releases_lease_and_restarts_idle_ttl() {
    let reg = Arc::new(TxRegistry::new());
    let ttl = Duration::from_secs(30);
    let (handle, it) = make_tx(SID_A, Duration::from_secs(300), 1).await;
    reg.register(handle, it).unwrap();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let task_reg = Arc::clone(&reg);
    let task = tokio::spawn(async move {
        let (_it, _lease) = task_reg.acquire_owned(handle, &SID_A).unwrap();
        ready_tx.send(()).unwrap();
        std::future::pending::<()>().await;
    });
    ready_rx.await.unwrap();
    tokio::time::advance(Duration::from_secs(61)).await;
    assert_eq!(reg.reap_expired(ttl), 0);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::advance(Duration::from_secs(29)).await;
    assert_eq!(reg.reap_expired(ttl), 0);
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(reg.reap_expired(ttl), 1);
    assert!(reg.is_empty());
}

#[tokio::test(start_paused = true)]
async fn absolute_lifetime_reaps_even_with_active_lease() {
    let reg = TxRegistry::new();
    let (handle, it) = make_tx(SID_A, Duration::from_secs(60), 1).await;
    reg.register(handle, it).unwrap();
    let (_it, lease) = reg.acquire_owned(handle, &SID_A).unwrap();
    tokio::time::advance(Duration::from_secs(59)).await;
    assert_eq!(reg.reap_expired(Duration::from_secs(30)), 0);
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(reg.reap_expired(Duration::from_secs(30)), 1);
    assert!(matches!(
        reg.acquire_owned(handle, &SID_A),
        Err(TxRegistryError::TxNotFound),
    ));
    drop(lease);
    assert!(
        reg.is_empty(),
        "lease release cannot resurrect a reaped entry",
    );
}

#[tokio::test(start_paused = true)]
async fn stale_reap_candidate_rechecks_lease_activity_and_identity() {
    let reg = TxRegistry::new();
    let ttl = Duration::from_secs(30);
    let (handle, it) = make_tx(SID_A, Duration::from_secs(300), 1).await;
    let candidate = reg.register(handle, it).unwrap();
    tokio::time::advance(ttl).await;
    assert_eq!(
        reg.expired_handles(tokio::time::Instant::now().into_std(), ttl),
        vec![handle],
    );
    let (_, lease) = reg.acquire_owned(handle, &SID_A).unwrap();
    assert!(reg
        .remove_expired(handle, &candidate, Duration::ZERO)
        .is_none());
    drop(lease);
    assert!(reg.remove_expired(handle, &candidate, ttl).is_none());
    reg.remove(handle).unwrap();
    assert!(matches!(
        reg.acquire_owned(handle, &SID_A),
        Err(TxRegistryError::TxNotFound),
    ));
    let (_, replacement) = make_tx(SID_A, Duration::from_secs(300), handle).await;
    reg.register(handle, replacement).unwrap();
    tokio::time::advance(ttl).await;
    assert!(reg.remove_expired(handle, &candidate, ttl).is_none());
    assert_eq!(
        reg.reap_expired(ttl),
        1,
        "only the current entry may be reaped",
    );
}

// ---------------------------------------------------------------------
// R4b-1 — activity mirror, commit observation guard, duplicate-handle
// fail-closed, gauge mirrors. Tx IDs are minted per repo, so the tests
// below reuse ONE handle across registers to model the collision the
// server-level registry can really see.

use crate::tx_registry::{TxActivityState, TxCommitGuard};

/// Register `it` under `handle` with activity attribution (handler shape).
fn register_activity(
    reg: &TxRegistry,
    handle: u64,
    it: InteractiveTx,
    session_handle: u64,
) -> Arc<InteractiveTx> {
    reg.register_activity(handle, it, session_handle, Arc::from("operator"))
        .expect("registration succeeds")
}

#[tokio::test]
async fn duplicate_handle_fails_closed_and_keeps_first_row() {
    let reg = TxRegistry::new();
    let (handle, first) = make_tx(SID_A, Duration::from_secs(300), 1).await;
    register_activity(&reg, handle, first, 7);

    // A second repo mints the same id: the registry must REJECT, never
    // overwrite (the old `DashMap::insert` silently replaced the row).
    let (_, second) = make_tx(SID_B, Duration::from_secs(300), 2).await;
    assert!(matches!(
        reg.register_activity(handle, second, 9, Arc::from("other")),
        Err(TxRegistryError::TxHandleCollision)
    ));

    // First transaction untouched, mirrors agree, no stray rows.
    assert!(reg.get_owned(handle, &SID_A).is_ok());
    assert_eq!(reg.len(), 1);
    assert_eq!(reg.activity_count(), 1);
    let row = reg.activity_row(handle).expect("projection row present");
    assert_eq!(row.session_handle(), 7);
    assert_eq!(row.username(), "operator");
    assert_eq!(row.state(), TxActivityState::Active);

    // SID_B's session slot was never taken by the rejected register.
    let (h2, third) = make_tx(SID_B, Duration::from_secs(300), 3).await;
    reg.register(h2, third).expect("session B slot still free");
    assert_eq!(reg.len(), 2);
    assert_eq!(reg.activity_count(), 2);
}

#[tokio::test]
async fn commit_guard_keeps_awaiting_commit_row_until_drop() {
    let reg = Arc::new(TxRegistry::new());
    let (handle, it) = make_tx(SID_A, Duration::from_secs(300), 1).await;
    let arc = register_activity(&reg, handle, it, 11);

    let guard: TxCommitGuard = reg.begin_commit(handle, &arc).expect("commit observed");

    // The tx is no longer executable: identity parked as `Committing` in
    // the authoritative map (re-register, acquire and reap all skip it),
    // and the open mirror lost it — but the activity mirror keeps the row.
    assert!(matches!(
        reg.get_owned(handle, &SID_A),
        Err(TxRegistryError::TxNotFound)
    ));
    assert!(matches!(
        reg.acquire_owned(handle, &SID_A),
        Err(TxRegistryError::TxNotFound)
    ));
    assert_eq!(reg.len(), 0, "committing row left the open gauge");
    assert_eq!(reg.activity_count(), 1, "committing row stays visible");
    let row = reg.activity_row(handle).expect("row visible mid-commit");
    assert_eq!(row.state(), TxActivityState::AwaitingCommit);
    assert_eq!(row.session_handle(), 11);
    assert_eq!(row.db(), "db");

    // The reaper cannot touch a committing row (identity parked as
    // `Committing`, skipped by the sweep). Zero-TTL sweep — asserted BEFORE
    // any replacement registration below: a freshly registered tx has no
    // activity bump yet, so a zero idle TTL would flag it immediately and
    // the assertion would race the replacement, not the committing row.
    assert!(reg
        .expired_handles(Instant::now(), Duration::ZERO)
        .is_empty());

    // The one-tx-per-session slot was freed at begin_commit.
    let (h2, next) = make_tx(SID_A, Duration::from_secs(300), 2).await;
    reg.register(h2, next)
        .expect("session slot freed while committing");

    // Guard drop = commit pipeline returned: projection row goes, activity
    // gauge follows.
    drop(guard);
    assert!(reg.activity_row(handle).is_none());
    assert_eq!(reg.activity_count(), 1, "only the re-registered row");
    assert_eq!(reg.activity_count(), reg.len());
}

#[tokio::test]
async fn commit_guard_reaped_race_reports_none() {
    let reg = Arc::new(TxRegistry::new());
    // Zero lifetime: the reaper fires between get_owned and begin_commit.
    let (handle, it) = make_tx(SID_A, Duration::ZERO, 1).await;
    let arc = register_activity(&reg, handle, it, 3);
    // Simulate the race: sweep removed the row; begin_commit must refuse
    // (identity recheck) and leave nothing behind.
    assert!(reg.remove(handle).is_some());
    assert!(reg.begin_commit(handle, &arc).is_none());
    assert_eq!(reg.activity_count(), 0);
}

#[tokio::test]
async fn expired_row_reports_expired_pending_reap_while_registered() {
    let reg = TxRegistry::new();
    let (handle, it) = make_tx(SID_A, Duration::ZERO, 1).await;
    register_activity(&reg, handle, it, 5);

    let row = reg.activity_row(handle).expect("row present");
    assert_eq!(
        row.state(),
        TxActivityState::ExpiredPendingReap,
        "real expiry must be reported while the row is still registered"
    );
    // The sweep then removes both the lifecycle row and the projection.
    assert_eq!(reg.reap_expired(Duration::from_secs(60)), 1);
    assert!(reg.activity_row(handle).is_none());
    assert_eq!(reg.activity_count(), 0);
    assert_eq!(reg.len(), 0);
}

#[tokio::test(start_paused = true)]
async fn projection_timing_is_shared_not_duplicated() {
    let reg = TxRegistry::new();
    let (handle, it) = make_tx(SID_A, Duration::from_secs(300), 1).await;
    let arc = register_activity(&reg, handle, it, 6);

    // Bump activity through the PARKED tx: the projection row must observe
    // it (one shared timing state, no drifting duplicate timestamp).
    //
    // Paused Tokio time: the registered-at instant is at elapsed zero, so
    // advance PAST it before bumping (7 ms) and again after (1 ms) — the
    // idle/elapsed split is then asserted at a DETERMINISTIC instant
    // (elapsed 8 ms, idle 1 ms since the bump) instead of racing a
    // real-clock threshold on a loaded CI runner. If the bump failed to
    // reach the projection row, idle would read 8 ms, not 1 ms.
    tokio::time::advance(Duration::from_millis(7)).await;
    arc.bump_activity();
    tokio::time::advance(Duration::from_millis(1)).await;
    let row = reg.activity_row(handle).expect("row present");
    assert_eq!(row.idle_ns(), 1_000_000, "idle clock follows the tx bump");
    assert_eq!(row.elapsed_ns(), 8_000_000);

    // Mirrors and rows agree on a multi-row registry.
    let (h2, it2) = make_tx(SID_B, Duration::from_secs(300), 2).await;
    register_activity(&reg, h2, it2, 8);
    assert_eq!(reg.len(), 2);
    assert_eq!(reg.activity_count(), 2);
    assert_eq!(reg.activity_rows().len(), 2);
    let mut handles: Vec<u64> = reg.activity_rows().iter().map(|(k, _)| *k).collect();
    handles.sort_unstable();
    assert_eq!(handles, vec![handle.min(h2), handle.max(h2)]);
}
