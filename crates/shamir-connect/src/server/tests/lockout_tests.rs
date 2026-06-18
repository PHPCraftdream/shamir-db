use crate::common::time::{ns, UnixNanos};
use crate::server::lockout::{
    subnet_of, username_hash, FailureOutcome, FailureState, InMemoryLockoutStore, LockoutSnapshot,
    LockoutSnapshotError, LockoutSnapshotSink, LockoutState, LockoutStore, PairKey, Subnet,
    LOCKOUT_DURATION_NS,
};
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::sync::Arc;

fn key(subnet: u8, user: u8) -> PairKey {
    (Subnet::V4([10, 0, subnet]), [user; 16])
}

#[test]
fn first_failure_returns_100ms_backoff() {
    let s = InMemoryLockoutStore::new();
    let now = 1_000_000_000;
    match s.register_failure(key(1, 1), now) {
        FailureOutcome::Backoff { delay_ms } => assert_eq!(delay_ms, 100),
        FailureOutcome::LockedOut => panic!("first failure must not lock out"),
    }
}

#[test]
fn backoff_doubles_per_failure() {
    let s = InMemoryLockoutStore::new();
    let now = 1_000_000_000;
    let k = key(1, 1);

    let expected = [
        100u64, 200, 400, 800, 1600, 3200, 6400, 12800, 25600, 30000, 30000,
    ];
    for (i, &want) in expected.iter().enumerate() {
        let got = match s.register_failure(k, now + (i as u64) * ns::SECOND) {
            FailureOutcome::Backoff { delay_ms } => delay_ms,
            FailureOutcome::LockedOut => 0,
        };
        assert_eq!(
            got,
            want,
            "failure #{} expected {}ms got {}ms",
            i + 1,
            want,
            got
        );
    }
}

#[test]
fn lockout_after_threshold() {
    let s = InMemoryLockoutStore::new();
    let now = 1_000_000_000;
    let k = key(1, 1);

    // 49 failures: still backoff.
    for i in 0..49 {
        let outcome = s.register_failure(k, now + (i as u64) * ns::SECOND);
        assert!(matches!(outcome, FailureOutcome::Backoff { .. }));
    }

    // 50th failure: locked out.
    let outcome = s.register_failure(k, now + 49 * ns::SECOND);
    assert_eq!(outcome, FailureOutcome::LockedOut);
    assert!(s.is_locked_out(k, now + 49 * ns::SECOND));
    assert_eq!(s.total_lockouts(), 1);
}

#[test]
fn lockout_expires_after_duration() {
    let s = InMemoryLockoutStore::new();
    let now = 1_000_000_000;
    let k = key(1, 1);
    for i in 0..50 {
        s.register_failure(k, now + (i as u64) * ns::SECOND);
    }
    let trigger_ts = now + 49 * ns::SECOND;
    assert!(s.is_locked_out(k, now + 50 * ns::SECOND));

    // Expiry: triggered_at + duration. Just after that → unlocked.
    let after = trigger_ts + LOCKOUT_DURATION_NS + 1;
    assert!(!s.is_locked_out(k, after));
}

#[test]
fn reset_on_success_clears_failure_and_lockout() {
    let s = InMemoryLockoutStore::new();
    let now = 1_000_000_000;
    let k = key(1, 1);
    s.register_failure(k, now);
    s.register_failure(k, now);
    assert!(s.current_backoff_ms(k, now) > 0);

    s.reset_on_success(k);
    assert_eq!(s.current_backoff_ms(k, now), 0);
    assert!(!s.is_locked_out(k, now));
}

#[test]
fn backoff_resets_after_inactivity_window() {
    let s = InMemoryLockoutStore::new();
    let now = 1_000_000_000;
    let k = key(1, 1);
    s.register_failure(k, now);
    s.register_failure(k, now); // backoff = 200ms

    // 6 minutes later → BACKOFF_RESET_NS exceeded → next failure is treated as fresh
    let later = now + 6 * ns::MINUTE;
    let outcome = s.register_failure(k, later);
    assert_eq!(outcome, FailureOutcome::Backoff { delay_ms: 100 });
}

#[test]
fn admin_unlock_user_clears_all_subnets() {
    let s = InMemoryLockoutStore::new();
    let now = 1_000_000_000;
    let user = [0xaau8; 16];
    let k1 = (Subnet::V4([10, 0, 1]), user);
    let k2 = (Subnet::V4([10, 0, 2]), user);
    for _ in 0..50 {
        s.register_failure(k1, now);
    }
    s.register_failure(k2, now);
    assert!(s.is_locked_out(k1, now));
    assert!(s.current_backoff_ms(k2, now) > 0);

    s.admin_unlock_user(user);
    assert!(!s.is_locked_out(k1, now));
    assert_eq!(s.current_backoff_ms(k2, now), 0);
}

#[test]
fn gc_removes_stale_entries() {
    let s = InMemoryLockoutStore::new();
    let now = 1_000_000_000;
    let k = key(1, 1);
    s.register_failure(k, now);
    assert_eq!(s.failure_pair_count(), 1);

    let later = now + 6 * ns::MINUTE;
    s.gc(later);
    assert_eq!(s.failure_pair_count(), 0);
}

#[test]
fn subnet_of_v4_takes_24_bit_prefix() {
    let s = subnet_of(IpAddr::V4(Ipv4Addr::new(10, 0, 1, 200)));
    assert_eq!(s, Subnet::V4([10, 0, 1]));
}

#[test]
fn username_hash_is_deterministic() {
    let secret = [0xaau8; 32];
    let h1 = username_hash(&secret, b"alice");
    let h2 = username_hash(&secret, b"alice");
    assert_eq!(h1, h2);

    let h3 = username_hash(&secret, b"bob");
    assert_ne!(h1, h3);
}

#[test]
fn username_hash_separates_lockout_from_server_secret() {
    // Different lockout_secrets must produce different hashes for the
    // same username — defends against secret-rotation orphan state.
    let h1 = username_hash(&[0x01u8; 32], b"alice");
    let h2 = username_hash(&[0x02u8; 32], b"alice");
    assert_ne!(h1, h2);
}

// -------------------------------------------------------------------
// Snapshot persistence tests (Option A — periodic durable snapshot).
// -------------------------------------------------------------------

/// In-memory sink for snapshot round-trip tests. Stores the latest
/// saved snapshot in a `Mutex<Option<…>>`.
struct MemSink(std::sync::Mutex<Option<LockoutSnapshot>>);
impl MemSink {
    fn new() -> Arc<Self> {
        Arc::new(Self(std::sync::Mutex::new(None)))
    }
    fn preload(snap: LockoutSnapshot) -> Arc<Self> {
        Arc::new(Self(std::sync::Mutex::new(Some(snap))))
    }
}
impl LockoutSnapshotSink for MemSink {
    fn save(&self, snapshot: &LockoutSnapshot) -> Result<(), LockoutSnapshotError> {
        *self.0.lock().unwrap() = Some(snapshot.clone());
        Ok(())
    }
    fn load(&self) -> Result<Option<LockoutSnapshot>, LockoutSnapshotError> {
        Ok(self.0.lock().unwrap().clone())
    }
}

#[test]
fn snapshot_captures_failures_and_lockouts() {
    let s = InMemoryLockoutStore::new();
    let now = 1_000_000_000;
    s.register_failure(key(1, 1), now);
    s.register_failure(key(1, 1), now);
    s.register_failure(key(2, 7), now);

    let snap = s.snapshot();
    assert_eq!(snap.failures.len(), 2);
    assert!(snap.lockouts.is_empty());

    // Drive one pair into LockedOut.
    for i in 0..50 {
        s.register_failure(key(3, 9), now + (i as u64) * ns::SECOND);
    }
    let snap2 = s.snapshot();
    assert_eq!(snap2.lockouts.len(), 1);
    assert!(snap2.total_lockouts >= 1);
}

#[test]
fn lockout_state_survives_snapshot_roundtrip() {
    // Use wall-clock-ish timestamps so the snapshot's
    // `captured_at_ns = UnixNanos::now()` doesn't make the rehydrated
    // entries look stale.
    let s = InMemoryLockoutStore::new();
    let now = UnixNanos::now().as_u64();
    let k = key(1, 1);

    // Drive to lockout.
    for i in 0..50 {
        s.register_failure(k, now + (i as u64) * ns::SECOND);
    }
    assert!(s.is_locked_out(k, now + 49 * ns::SECOND));

    // Round-trip through msgpack (the durable encoding used by
    // the redb-backed sink in shamir-server).
    let snap = s.snapshot();
    let bytes = rmp_serde::to_vec_named(&snap).expect("encode");
    let restored: LockoutSnapshot = rmp_serde::from_slice(&bytes).expect("decode");

    let s2 = InMemoryLockoutStore::with_snapshot(restored);
    // Probe at the capture instant (well inside lockout duration).
    let probe_ns = snap.captured_at_ns;
    assert!(
        s2.is_locked_out(k, probe_ns),
        "lockout must survive snapshot round-trip"
    );
    assert!(s2.total_lockouts() >= 1);
}

#[test]
fn failure_backoff_survives_snapshot_roundtrip() {
    let s = InMemoryLockoutStore::new();
    // Use wall-clock-ish timestamps so rehydrate's freshness check
    // does not discard the entry.
    let now = UnixNanos::now().as_u64();
    let k = key(1, 1);
    s.register_failure(k, now);
    s.register_failure(k, now);
    let pre_backoff = s.current_backoff_ms(k, now);
    assert!(pre_backoff > 0);

    let snap = s.snapshot();
    let bytes = rmp_serde::to_vec_named(&snap).expect("encode");
    let restored: LockoutSnapshot = rmp_serde::from_slice(&bytes).expect("decode");

    let s2 = InMemoryLockoutStore::with_snapshot(restored);
    // At the captured instant the backoff requirement is preserved
    // verbatim (count and last_fail_at_ns both round-trip).
    let probe_ns = snap.captured_at_ns;
    assert_eq!(s2.current_backoff_ms(k, probe_ns), pre_backoff);
}

#[test]
fn rehydrate_drops_stale_failures_and_expired_lockouts() {
    // Capture a snapshot in the past, then rehydrate as-if at that
    // point: stale entries (older than BACKOFF_RESET_NS at capture)
    // are dropped, expired lockouts likewise.
    //
    // `captured_at_ns` must be > LOCKOUT_DURATION_NS so the "expired
    // lockout" arithmetic stays in range.
    let captured_at_ns = 10 * ns::HOUR;
    let stale_ts = captured_at_ns - 10 * ns::MINUTE; // > BACKOFF_RESET_NS old
    let fresh_ts = captured_at_ns - ns::MINUTE;

    let snap = LockoutSnapshot {
        failures: vec![
            // Stale: > BACKOFF_RESET_NS old at capture.
            (
                key(1, 1),
                FailureState {
                    count: 3,
                    last_fail_at_ns: stale_ts,
                },
            ),
            // Fresh: still within window.
            (
                key(2, 2),
                FailureState {
                    count: 2,
                    last_fail_at_ns: fresh_ts,
                },
            ),
        ],
        lockouts: vec![
            // Expired: triggered LOCKOUT_DURATION_NS + 1s ago.
            (
                key(3, 3),
                LockoutState {
                    triggered_at_ns: captured_at_ns - LOCKOUT_DURATION_NS - ns::SECOND,
                    duration_ns: LOCKOUT_DURATION_NS,
                },
            ),
            // Still active.
            (
                key(4, 4),
                LockoutState {
                    triggered_at_ns: captured_at_ns - ns::MINUTE,
                    duration_ns: LOCKOUT_DURATION_NS,
                },
            ),
        ],
        total_lockouts: 42,
        captured_at_ns,
    };

    let s = InMemoryLockoutStore::with_snapshot(snap);
    assert_eq!(s.failure_pair_count(), 1, "stale failure must be dropped");
    assert_eq!(
        s.active_lockout_count(),
        1,
        "expired lockout must be dropped"
    );
    // Metric is preserved verbatim — it's a counter, not a real-time
    // decision input.
    assert_eq!(s.total_lockouts(), 42);
}

#[test]
fn with_snapshot_sink_rehydrates_from_sink() {
    // Construct a synthetic snapshot, install it in a sink, then a
    // fresh store created `with_snapshot_sink` must mirror it.
    let now = 5_000_000_000;
    let k = key(7, 7);
    let snap = LockoutSnapshot {
        failures: vec![(
            k,
            FailureState {
                count: 4,
                last_fail_at_ns: now,
            },
        )],
        lockouts: vec![],
        total_lockouts: 0,
        captured_at_ns: now,
    };
    let sink = MemSink::preload(snap);

    let s = InMemoryLockoutStore::with_snapshot_sink(sink);
    // count=4 → backoff = 100 * 2^3 = 800
    assert_eq!(s.current_backoff_ms(k, now), 800);
}

#[test]
fn persist_snapshot_writes_through_sink() {
    let sink = MemSink::new();
    let s = InMemoryLockoutStore::with_snapshot_sink(sink.clone());
    s.register_failure(key(1, 1), 1_000_000_000);
    s.register_failure(key(1, 1), 1_000_000_000);

    let wrote = s.persist_snapshot().expect("persist must succeed");
    assert!(wrote, "sink installed → persist returns true");

    let stored = sink.0.lock().unwrap().clone().expect("snapshot stored");
    assert_eq!(stored.failures.len(), 1);
    assert_eq!(stored.failures[0].1.count, 2);
}

#[test]
fn persist_snapshot_without_sink_is_noop() {
    let s = InMemoryLockoutStore::new();
    s.register_failure(key(1, 1), 1_000_000_000);
    let wrote = s.persist_snapshot().expect("noop must succeed");
    assert!(!wrote, "no sink → persist returns false");
}

#[test]
fn snapshot_second_codec_roundtrip() {
    // Belt-and-suspenders: the docs promise the snapshot is
    // serde-compatible. Verify against a second codec (rmp named encoding)
    // so a codec-specific quirk can't mask a missing derive.
    let s = InMemoryLockoutStore::new();
    s.register_failure(key(1, 1), 1_000_000_000);
    let snap = s.snapshot();
    let bytes = rmp_serde::to_vec_named(&snap).expect("encode");
    let restored: LockoutSnapshot = rmp_serde::from_slice(&bytes).expect("decode");
    assert_eq!(restored.failures.len(), snap.failures.len());
    assert_eq!(restored.total_lockouts, snap.total_lockouts);
}
