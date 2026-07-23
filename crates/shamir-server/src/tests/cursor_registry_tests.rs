//! Unit tests for `crate::cursor_registry` — `Cursor`/`CursorRegistry`
//! register/get/remove, per-session cap, and idle-timeout eviction
//! semantics, in isolation from the wire handler (FG-5b).

use std::sync::Arc;
use std::time::{Duration, Instant};

use shamir_query_types::read::ReadQuery;

use crate::cursor_registry::{
    spawn_reaper_task, Cursor, CursorRegistry, CursorRegistryError, PaginationMode,
};

const SID_A: [u8; 32] = [0xAA; 32];
const SID_B: [u8; 32] = [0xBB; 32];

/// Build a real `Cursor` from a standalone gate — mirrors
/// `tx_registry_tests::make_tx`'s "genuine `SnapshotGuard`" fixture style.
async fn make_cursor(seed: u64) -> Cursor {
    let gate = shamir_tx::RepoTxGate::new(0, seed);
    let guard = gate.open_snapshot().await;
    let version = guard.version();
    Cursor::new(
        ReadQuery::new("items"),
        PaginationMode::Offset,
        guard,
        version,
        10,
        SID_A,
        "db".to_string(),
        "repo".to_string(),
    )
}

#[tokio::test]
async fn register_then_get_owned_roundtrip() {
    let reg = CursorRegistry::new();
    let cursor = make_cursor(1).await;
    reg.register(1, SID_A, cursor, 16).unwrap();

    let got = reg.get_owned(1, &SID_A).unwrap();
    assert_eq!(got.db(), "db");
    assert_eq!(got.repo(), "repo");
    assert_eq!(reg.len(), 1);
    assert_eq!(reg.open_count_for_session(&SID_A), 1);
}

#[tokio::test]
async fn get_owned_unknown_id_returns_not_found() {
    let reg = CursorRegistry::new();
    assert!(matches!(
        reg.get_owned(999, &SID_A),
        Err(CursorRegistryError::CursorNotFound)
    ));
}

#[tokio::test]
async fn get_owned_foreign_session_rejected() {
    let reg = CursorRegistry::new();
    let cursor = make_cursor(1).await;
    reg.register(1, SID_A, cursor, 16).unwrap();

    assert!(matches!(
        reg.get_owned(1, &SID_B),
        Err(CursorRegistryError::CursorOwnershipMismatch)
    ));
}

#[tokio::test]
async fn remove_frees_session_slot_for_a_new_cursor() {
    let reg = CursorRegistry::new();
    let cursor = make_cursor(1).await;
    reg.register(1, SID_A, cursor, 1).unwrap();
    assert_eq!(reg.open_count_for_session(&SID_A), 1);

    let removed = reg.remove(1).expect("present");
    assert_eq!(removed.owner_sid(), &SID_A);
    assert!(reg.is_empty());
    assert_eq!(reg.open_count_for_session(&SID_A), 0);

    // Slot freed -> a new cursor can be registered even at cap=1.
    let cursor2 = make_cursor(2).await;
    reg.register(2, SID_A, cursor2, 1).unwrap();
    assert_eq!(reg.len(), 1);
}

/// Per-session cap rejection: the (cap+1)-th `register` on one session is
/// rejected with `CursorLimitExceeded`, and the rejected cursor leaves no
/// trace (its `SnapshotGuard` — held only by the dropped `Cursor` — is
/// released, not leaked in the registry).
#[tokio::test]
async fn register_respects_per_session_cap_and_rejects_past_it() {
    let reg = CursorRegistry::new();
    let cap = 3u32;
    for i in 0..cap as u64 {
        let cursor = make_cursor(i + 1).await;
        reg.register(i + 1, SID_A, cursor, cap).unwrap();
    }
    assert_eq!(reg.open_count_for_session(&SID_A), cap as usize);

    // The next one is rejected.
    let over_cap = make_cursor(999).await;
    let err = reg.register(999, SID_A, over_cap, cap);
    assert!(matches!(
        err,
        Err(CursorRegistryError::CursorLimitExceeded { limit }) if limit == cap
    ));

    // No trace of the rejected cursor: count unchanged, id not present.
    assert_eq!(reg.open_count_for_session(&SID_A), cap as usize);
    assert!(reg.get_owned(999, &SID_A).is_err());
    assert_eq!(reg.len(), cap as usize);
}

/// A different session is unaffected by another session's cap.
#[tokio::test]
async fn per_session_cap_is_scoped_per_session() {
    let reg = CursorRegistry::new();
    let cap = 1u32;
    let c1 = make_cursor(1).await;
    reg.register(1, SID_A, c1, cap).unwrap();

    // SID_A is at cap, but SID_B has its own independent slot.
    let c2 = Cursor::new(
        ReadQuery::new("items"),
        PaginationMode::Offset,
        shamir_tx::RepoTxGate::new(0, 2).open_snapshot().await,
        0,
        10,
        SID_B,
        "db".to_string(),
        "repo".to_string(),
    );
    reg.register(2, SID_B, c2, cap).unwrap();
    assert_eq!(reg.len(), 2);
}

/// `expired_ids` correctly identifies an idle-past-TTL cursor (activity
/// bump defers it, mirroring `tx_registry_tests`'s idle-ttl coverage).
#[tokio::test]
async fn expired_ids_identifies_idle_past_ttl_cursor() {
    let reg = CursorRegistry::new();
    let cursor = make_cursor(1).await;
    let arc = reg.register(1, SID_A, cursor, 16).unwrap();

    // Zero idle-ttl reaps any inactive cursor immediately.
    let expired = reg.expired_ids(Instant::now(), Duration::ZERO);
    assert_eq!(expired, vec![1]);

    // Bump activity — with a generous TTL it's no longer expired.
    arc.bump_activity();
    assert!(reg
        .expired_ids(Instant::now(), Duration::from_secs(3600))
        .is_empty());
}

/// Sweep workflow: `expired_ids` yields the reaped set; `remove_for_idle_reap`
/// drops the `Cursor` (RAII release of the `SnapshotGuard`) and tombstones
/// the id so a racing `FetchNext` sees `CursorExpired`, not `CursorNotFound`.
#[tokio::test]
async fn reap_tombstones_id_so_later_lookup_reports_expired_not_not_found() {
    let reg = CursorRegistry::new();
    let cursor = make_cursor(1).await;
    reg.register(1, SID_A, cursor, 16).unwrap();

    let expired = reg.expired_ids(Instant::now(), Duration::ZERO);
    assert_eq!(expired, vec![1]);
    for id in expired {
        let arc = reg.remove_for_idle_reap(id);
        assert!(
            arc.is_some(),
            "remove yields the parked cursor for RAII drop"
        );
    }
    assert!(reg.is_empty());

    // The distinguishing behavior: a later FetchNext-style lookup against
    // the reaped id reports CursorExpired (idle-timeout), not the generic
    // CursorNotFound a never-issued id would get.
    assert!(matches!(
        reg.get_owned(1, &SID_A),
        Err(CursorRegistryError::CursorExpired)
    ));

    // A genuinely never-issued id still reports CursorNotFound.
    assert!(matches!(
        reg.get_owned(42, &SID_A),
        Err(CursorRegistryError::CursorNotFound)
    ));

    // Session slot freed after the reap -> a new cursor can be opened.
    let cursor2 = make_cursor(2).await;
    reg.register(2, SID_A, cursor2, 16).unwrap();
    assert_eq!(reg.len(), 1);
}

/// Explicit `remove` (CancelCursor path) does NOT tombstone — a later
/// lookup against a deliberately-canceled id is a plain `CursorNotFound`,
/// not `CursorExpired`.
#[tokio::test]
async fn explicit_remove_does_not_tombstone() {
    let reg = CursorRegistry::new();
    let cursor = make_cursor(1).await;
    reg.register(1, SID_A, cursor, 16).unwrap();

    reg.remove(1).expect("present");
    assert!(matches!(
        reg.get_owned(1, &SID_A),
        Err(CursorRegistryError::CursorNotFound)
    ));
}

/// CR-B8 (#774, R-3): `free_session_slot` must not merely zero the
/// session's counter — once the live count reaches zero, the `by_session`
/// entry itself must be removed, or a long-lived server leaks one
/// `[u8; 32] -> Arc<AtomicUsize>` entry per connection that ever opened a
/// cursor. Covers both removal paths: explicit `remove` (CancelCursor) and
/// `remove_for_idle_reap`.
#[tokio::test]
async fn by_session_entry_removed_after_explicit_remove_drains_to_zero() {
    let reg = CursorRegistry::new();
    let cursor = make_cursor(1).await;
    reg.register(1, SID_A, cursor, 16).unwrap();
    assert_eq!(reg.by_session_len(), 1);

    reg.remove(1).expect("present");
    assert_eq!(
        reg.open_count_for_session(&SID_A),
        0,
        "count reads 0 via the map-miss fallback"
    );
    assert_eq!(
        reg.by_session_len(),
        0,
        "the by_session entry itself must be gone, not merely zeroed"
    );
}

#[tokio::test]
async fn by_session_entry_removed_after_idle_reap_drains_to_zero() {
    let reg = CursorRegistry::new();
    let cursor = make_cursor(1).await;
    reg.register(1, SID_A, cursor, 16).unwrap();
    assert_eq!(reg.by_session_len(), 1);

    let expired = reg.expired_ids(Instant::now(), Duration::ZERO);
    assert_eq!(expired, vec![1]);
    for id in expired {
        reg.remove_for_idle_reap(id);
    }

    assert_eq!(reg.open_count_for_session(&SID_A), 0);
    assert_eq!(
        reg.by_session_len(),
        0,
        "the by_session entry itself must be gone, not merely zeroed"
    );
}

/// Concurrency stress test proving the per-session cap is never violated
/// and the `by_session` entry fully drains, across many interleaved
/// concurrent register/remove cycles for the SAME session id. This is the
/// test that would catch a TOCTOU bug in `free_session_slot`'s
/// decrement-and-maybe-remove: if a racing `register` and `free_session_slot`
/// interleaved unsafely, a session could end up exceeding `max_per_session`
/// split across two independently-created counters, or the final
/// `by_session` entry could linger non-zero / fail to be removed.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_register_remove_cycles_never_exceed_cap_and_fully_drain() {
    let reg = Arc::new(CursorRegistry::new());
    let cap: u32 = 4;
    let tasks_count = 32u64;
    let cycles_per_task = 25u64;

    let mut handles = Vec::new();
    for t in 0..tasks_count {
        let reg = Arc::clone(&reg);
        handles.push(tokio::spawn(async move {
            for c in 0..cycles_per_task {
                let seed = t * cycles_per_task + c + 1;
                let cursor = make_cursor(seed).await;
                let cursor_id = seed;
                match reg.register(cursor_id, SID_A, cursor, cap) {
                    Ok(_) => {
                        // Verify the cap is never exceeded from this
                        // vantage point (best-effort — other tasks may
                        // concurrently mutate the count, but it must
                        // never read ABOVE cap).
                        assert!(reg.open_count_for_session(&SID_A) <= cap as usize);
                        // Immediately remove so the slot frees up for
                        // other concurrent tasks (interleaves the
                        // zero-count-triggering remove with concurrent
                        // registers, per the brief).
                        reg.remove(cursor_id);
                    }
                    Err(CursorRegistryError::CursorLimitExceeded { limit }) => {
                        assert_eq!(limit, cap);
                    }
                    Err(other) => panic!("unexpected error: {other:?}"),
                }
            }
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    // Final state: fully drained. No open cursors, no lingering
    // by_session entry for this session (proves free_session_slot's
    // decrement-and-maybe-remove never lost a decrement nor left a
    // zero-count entry behind).
    assert!(reg.is_empty());
    assert_eq!(reg.open_count_for_session(&SID_A), 0);
    assert_eq!(
        reg.by_session_len(),
        0,
        "by_session must be fully drained once every cursor is removed"
    );
}

/// The background reaper task actually reaps an idle-past-TTL cursor on its
/// own schedule (paused virtual time — no real sleeping).
#[tokio::test(start_paused = true)]
async fn reaper_task_reaps_idle_cursor() {
    let reg = Arc::new(CursorRegistry::new());
    let cursor = make_cursor(1).await;
    reg.register(1, SID_A, cursor, 16).unwrap();
    assert_eq!(reg.len(), 1);

    let shutdown = tokio_util::sync::CancellationToken::new();
    // Zero idle-ttl: the cursor is idle-expired the instant any wall-clock
    // time has elapsed since creation — same "shrink the timeout under
    // test" trick `tx_registry_tests::reaper_task_reaps_past_deadline_tx`
    // uses (there via a zero absolute lifetime; here there is no separate
    // absolute deadline, only idle-ttl, so zero idle-ttl is the analogous
    // knob). This isolates the assertion to "does the reaper's sweep loop
    // actually fire and call remove_for_idle_reap", not to a specific idle
    // duration.
    let reaper = spawn_reaper_task(
        Arc::clone(&reg),
        Duration::ZERO,
        Duration::from_millis(50),
        shutdown.clone(),
    );

    // First interval tick is dropped by the reaper on boot; the second is
    // the real sweep. Advance in two steps, deterministic under paused time.
    tokio::time::advance(Duration::from_millis(50)).await;
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(50)).await;
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;

    assert!(reg.is_empty(), "reaper task drained the idle cursor");
    assert!(matches!(
        reg.get_owned(1, &SID_A),
        Err(CursorRegistryError::CursorExpired)
    ));

    shutdown.cancel();
    let _ = reaper.handle.await;
}
