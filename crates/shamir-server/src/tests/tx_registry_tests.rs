use std::sync::Arc;
use std::time::{Duration, Instant};

use shamir_db::engine::repo::{BoxRepo, RepoInstance};
use shamir_db::engine::table::TableConfig;
use shamir_db::storage::storage_in_memory::InMemoryRepo;
use shamir_types::types::value::InnerValue;

use crate::tx_registry::{spawn_reaper_task, InteractiveTx, TxRegistry, TxRegistryError};

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
    );
    (tx_id.0, it)
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
