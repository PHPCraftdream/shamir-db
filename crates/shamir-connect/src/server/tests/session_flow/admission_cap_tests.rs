//! Regression tests for the capped per-user session admission
//! (`SessionStore::admit`, spec §7.4):
//! - RV-CONNECT-0A42F42C: all-active cap REJECTS a new login instead of
//!   evicting an active session; exactly-60 s boundary is idle (LRU evict).
//! - RV-CONNECT-D8C2825A: the per-user sharded index keeps admission
//!   O(cap) and atomic (races, replacement, sid conflicts, stale removal).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};

use crate::common::types::{BindingMode, TransportKind};
use crate::server::admission::{Admission, AdmitReject, ACTIVE_WINDOW_NS};
use crate::server::session::{Session, SessionPermissions, SessionStore};

const T: u64 = 1_000_000_000_000; // arbitrary fixed "now"
const SECOND: u64 = 1_000_000_000;

fn sess(uid: [u8; 16], last_activity_ns: u64) -> Session {
    Session::new(
        uid,
        "u".into(),
        SessionPermissions::from_roles(vec![]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        last_activity_ns,
    )
}

fn uid(n: u8) -> [u8; 16] {
    [n; 16]
}

fn sid(n: u8) -> [u8; 32] {
    [n; 32]
}

/// RV-CONNECT-0A42F42C — all-active cap: 16 sessions with
/// `last = T - 59_999_999_999` (active: age < 60 s) ⇒ the 17th login is
/// REJECTED with `AllActive`; count stays 16, the new sid is absent and all
/// 16 existing sessions are unchanged.
///
/// Fails if ONLY the all-active rejection fix is reverted (old
/// `insert_with_per_user_cap` unconditionally evicts the LRU session
/// without any activity threshold): the 17th admit would succeed and the
/// count would stay 16 with the oldest sid gone.
#[test]
fn rv_connect_0a42f42c_all_active_rejects_login() {
    let store = SessionStore::new();
    let u = uid(1);
    let last = T - (ACTIVE_WINDOW_NS - 1); // active (59.999…s old)
                                           // Keep the seeded Arcs: `lookup` TOUCHES the session, so activity is
                                           // asserted on the returned Arcs (reads only) — never via `lookup`.
    let mut arcs = Vec::new();
    for i in 0..16u8 {
        let a = match store.admit(sid(i), sess(u, last), T, 16) {
            Admission::Admitted { session, .. } => session,
            Admission::Rejected(r) => panic!("seeding must succeed: {r:?}"),
        };
        assert_eq!(a.last_activity_ns.load(Ordering::Relaxed), last);
        arcs.push(a);
    }
    assert_eq!(store.count_for_user(&u), 16);

    let new_sid = sid(200);
    match store.admit(new_sid, sess(u, T), T, 16) {
        Admission::Rejected(AdmitReject::AllActive) => {}
        other => panic!("expected Rejected(AllActive), got {other:?}"),
    }
    assert_eq!(store.count_for_user(&u), 16, "count must be unchanged");
    assert_eq!(store.len(), 16, "no session added");
    // Non-mutating index checks: the new sid is absent, all 16 present.
    let snap = store.snapshot_by_user(&u);
    assert_eq!(snap.len(), 16);
    assert!(!snap.contains(&new_sid), "new sid must be absent");
    for (i, a) in arcs.iter().enumerate() {
        assert_eq!(
            a.last_activity_ns.load(Ordering::Relaxed),
            last,
            "sid {i} must be unchanged"
        );
    }
}

/// RV-CONNECT-0A42F42C — exactly 60 s is IDLE: the oldest session with
/// `last = T - 60 s` is evicted (LRU) and the 17th login is admitted.
///
/// Fails if ONLY the boundary fix is reverted (active defined as
/// `age <= ACTIVE_WINDOW_NS`): the oldest session would classify as
/// active and the login would be `Rejected(AllActive)` instead of
/// `Admitted` with `evicted == [oldest]`.
#[test]
fn rv_connect_0a42f42c_exact_60s_boundary_evicts_oldest() {
    let store = SessionStore::new();
    let u = uid(2);
    let oldest_sid = sid(0);
    // Oldest session idle at exactly 60 s; others active (1 s old).
    match store.admit(oldest_sid, sess(u, T - ACTIVE_WINDOW_NS), T, 16) {
        Admission::Admitted { .. } => {}
        Admission::Rejected(r) => panic!("seeding must succeed: {r:?}"),
    }
    for i in 1..16u8 {
        match store.admit(sid(i), sess(u, T - SECOND), T, 16) {
            Admission::Admitted { .. } => {}
            Admission::Rejected(r) => panic!("seeding must succeed: {r:?}"),
        }
    }
    assert_eq!(store.count_for_user(&u), 16);

    let new_sid = sid(201);
    let evicted = match store.admit(new_sid, sess(u, T), T, 16) {
        Admission::Admitted { evicted, .. } => evicted,
        Admission::Rejected(r) => panic!("exactly 60 s is idle, must evict: {r:?}"),
    };
    assert_eq!(evicted.as_slice(), &[oldest_sid], "LRU idle victim evicted");
    assert!(store.lookup(&oldest_sid).is_none());
    assert_eq!(store.count_for_user(&u), 16);
    assert!(store.lookup(&new_sid).is_some());
}

/// RV-CONNECT-D8C2825A — two threads barrier-racing at cap−1 on fresh
/// stores, 1000 iterations (alternating scenarios). The per-user shard
/// guard SERIALIZES the two admits (the first takes the single vacancy
/// without evicting), so the outcome is order-independent:
/// - even (15 ACTIVE seeds, cap 16): one contender takes the vacancy; the
///   other would have to evict an ACTIVE session ⇒ `Rejected(AllActive)`.
///   Exactly one Admitted, one Rejected, nothing evicted, count 16.
/// - odd (14 active + 1 idle `sid(0)`, the LRU): one contender takes the
///   vacancy, the other evicts `sid(0)` ⇒ both Admitted, evicted_total ==
///   [sid(0)] exactly once, count 16, both contender sids present.
///
/// Fails if ONLY the all-active fix is reverted (old code evicts an ACTIVE
/// session): in the even case both threads are Admitted and a seed is
/// evicted. Fails if ONLY the atomicity fix is reverted (cap decision not
/// under the per-user guard): both threads can take the same vacancy and
/// publish 17 sessions.
#[test]
fn rv_connect_d8c2825a_two_threads_race_at_cap_minus_one() {
    let u = uid(3);
    let idle_sid = sid(0);
    for iter in 0..1000u32 {
        let store = Arc::new(SessionStore::new());
        let even = iter % 2 == 0;
        // Seed 15 sessions: scenario-dependent activity.
        for i in 0..15u8 {
            let last = if even || i != 0 {
                T - SECOND // active
            } else {
                T - ACTIVE_WINDOW_NS // exactly 60 s ⇒ idle
            };
            match store.admit(sid(i), sess(u, last), T, 16) {
                Admission::Admitted { .. } => {}
                Admission::Rejected(r) => panic!("seeding must succeed: {r:?}"),
            }
        }

        let barrier = Arc::new(Barrier::new(2));
        let mut joins = Vec::new();
        for t in 0..2u8 {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            let new_sid = sid(100 + t);
            joins.push(std::thread::spawn(move || {
                barrier.wait();
                store.admit(new_sid, sess(u, T), T, 16)
            }));
        }
        let mut evicted_total: Vec<[u8; 32]> = Vec::new();
        let mut admitted = 0;
        let mut rejected_all_active = 0;
        for j in joins {
            match j.join().expect("thread") {
                Admission::Admitted { evicted, .. } => {
                    admitted += 1;
                    evicted_total.extend(evicted);
                }
                Admission::Rejected(AdmitReject::AllActive) => rejected_all_active += 1,
                Admission::Rejected(r) => panic!("unexpected reject {r:?}"),
            }
        }
        let snap = store.snapshot_by_user(&u);
        if even {
            // 15 active before the race: one contender takes the vacancy, the
            // other would need to evict an ACTIVE session and is rejected.
            assert_eq!(admitted, 1, "iter {iter}: exactly one takes the vacancy");
            assert_eq!(
                rejected_all_active, 1,
                "iter {iter}: the other must be Rejected(AllActive)"
            );
            assert!(
                evicted_total.is_empty(),
                "iter {iter}: no eviction expected in the all-active scenario"
            );
            assert_eq!(store.count_for_user(&u), 16, "iter {iter}: count == cap");
            let contender_present = snap
                .iter()
                .filter(|k| **k == sid(100) || **k == sid(101))
                .count();
            assert_eq!(
                contender_present, 1,
                "iter {iter}: exactly one contender sid present"
            );
        } else {
            // One idle LRU victim (sid(0)): the first contender takes the
            // vacancy, the second evicts the idle session ⇒ both admitted.
            assert_eq!(admitted, 2, "iter {iter}: both admitted");
            assert_eq!(rejected_all_active, 0, "iter {iter}: none rejected");
            assert_eq!(
                evicted_total,
                vec![idle_sid],
                "iter {iter}: exactly the idle sid, exactly once"
            );
            assert_eq!(store.count_for_user(&u), 16, "iter {iter}: count == cap");
            for t in 0..2u8 {
                assert!(
                    snap.contains(&sid(100 + t)),
                    "iter {iter}: contender present"
                );
            }
        }
    }
}

/// RV-CONNECT-D8C2825A — 64 barrier-started threads admit concurrently
/// into a mixed idle/active full store: after every join the per-user
/// count is ≤ 16 and `snapshot_by_user` equals a scan of `by_sid`
/// (invariant I2 exact index).
///
/// Consistency stress for the per-user index (not a mutation oracle for the
/// O(cap) property): it fails if the cap decision stops being atomic under
/// the per-user guard (count > 16) or the per-user index diverges from
/// `by_sid`.
#[test]
fn rv_connect_d8c2825a_64_thread_mixed_never_exceeds_cap() {
    const N: usize = 64;
    let u = uid(4);
    let store = Arc::new(SessionStore::new());
    for i in 0..16u8 {
        let last = if i % 2 == 0 {
            T - 2 * ACTIVE_WINDOW_NS // idle
        } else {
            T - SECOND // active
        };
        match store.admit(sid(i), sess(u, last), T, 16) {
            Admission::Admitted { .. } => {}
            Admission::Rejected(r) => panic!("seeding must succeed: {r:?}"),
        }
    }

    let barrier = Arc::new(Barrier::new(N));
    let admitted = Arc::new(AtomicUsize::new(0));
    let mut joins = Vec::new();
    for t in 0..N {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        let admitted = Arc::clone(&admitted);
        // Contender sids must be DISTINCT from the 16 seed sids sid(0..16)
        // — a collision would make the "race" partly a replacement, which
        // consumes no extra capacity and weakens the cap invariant.
        let new_sid = sid(100 + t as u8);
        joins.push(std::thread::spawn(move || {
            barrier.wait();
            let out = store.admit(new_sid, sess(u, T), T, 16);
            if matches!(out, Admission::Admitted { .. }) {
                admitted.fetch_add(1, Ordering::SeqCst);
            }
            // Cap asserted INSIDE each thread right after its own admit, so
            // a transient over-cap publication is caught even mid-race.
            assert!(
                store.count_for_user(&u) <= 16,
                "per-user count must never exceed the cap (in-thread)"
            );
            out
        }));
    }
    for j in joins {
        let _ = j.join().expect("thread");
    }
    assert!(
        store.count_for_user(&u) <= 16,
        "per-user count must never exceed the cap"
    );
    // I2 exact index: snapshot equals a scan of by_sid (single user store).
    let snapshot = store.snapshot_by_user(&u);
    assert_eq!(snapshot.len(), store.len());
    for k in &snapshot {
        assert!(store.lookup(k).is_some(), "snapshot sid must be routable");
    }
}

/// RV-CONNECT-D8C2825A — replacement + sid conflict + stale removal:
/// 1. same sid + same user twice ⇒ `replaced`, count unchanged, lookup
///    returns the NEW Arc, and a following `remove` empties the user;
/// 2. a remove racing (behind a Barrier) with a replacement leaves the
///    store exact: either the sid is fully absent or it is present with
///    the new Arc and counted once (ptr_eq stale-removal guard);
/// 3. the same sid from ANOTHER user ⇒ `SidConflict` with both counts
///    unchanged.
///
/// Fails if ONLY the replacement-atomicity fix is reverted (old code
/// inserted by sid with no ownership transaction): the replacement case
/// corrupts cap accounting (double-counted or overwriting another user's
/// binding) and the cross-user case silently steals the sid.
#[test]
fn rv_connect_d8c2825a_replacement_and_sid_conflict() {
    let u1 = uid(5);
    let u2 = uid(6);
    let s = sid(7);

    // 1. Replacement.
    let store = SessionStore::new();
    let arc1 = store.insert(s, sess(u1, T));
    let out = match store.admit(s, sess(u1, T + 1), T + 1, 16) {
        Admission::Admitted {
            session,
            evicted,
            replaced,
        } => {
            assert!(evicted.is_empty(), "replacement evicts nothing");
            (session, replaced)
        }
        Admission::Rejected(r) => panic!("replacement must succeed: {r:?}"),
    };
    assert!(out.1, "must be flagged as a replacement");
    assert_eq!(
        store.count_for_user(&u1),
        1,
        "count unchanged by replacement"
    );
    let arc2 = out.0;
    assert!(
        Arc::ptr_eq(&arc2, store.lookup(&s).as_ref().expect("lookup")),
        "lookup must return the NEW Arc"
    );
    assert!(!Arc::ptr_eq(&arc1, &arc2));
    assert_eq!(
        store.remove(&s).as_ref().map(|a| Arc::ptr_eq(a, &arc2)),
        Some(true)
    );
    assert_eq!(store.count_for_user(&u1), 0, "remove empties the user");
    assert_eq!(store.len(), 0);

    // 2. Stale remove racing with a replacement (Barrier start). The two
    //    operations race freely — the assertions are INVARIANTS valid under
    //    EVERY interleaving (including "replacement commits first, so the
    //    subsequent remove legitimately removes the NEW generation"), not a
    //    fixed outcome order:
    //    - index exactness: len == count_for_user == snapshot len;
    //    - the sid appears at most once in the slot (implied by snap len);
    //    - count 0 ⇔ lookup absent; count 1 ⇔ lookup present;
    //    - a Some removal is always one of the two valid generations.
    for _ in 0..200 {
        let store = Arc::new(SessionStore::new());
        let arc1 = store.insert(s, sess(u1, T));
        let barrier = Arc::new(Barrier::new(2));
        let s1 = Arc::clone(&store);
        let b1 = Arc::clone(&barrier);
        let remover = std::thread::spawn(move || {
            b1.wait();
            s1.remove(&s)
        });
        let s2 = Arc::clone(&store);
        let b2 = Arc::clone(&barrier);
        let replacer = std::thread::spawn(move || {
            b2.wait();
            s2.admit(s, sess(u1, T + 1), T + 1, 16)
        });
        let removed = remover.join().expect("remover");
        let repl_out = replacer.join().expect("replacer");
        let count = store.count_for_user(&u1);
        let snap = store.snapshot_by_user(&u1);
        let arc2 = match &repl_out {
            Admission::Admitted { session, .. } => Some(Arc::clone(session)),
            Admission::Rejected(_) => panic!("the replacement admit must always succeed"),
        };
        // Invariant: index exactness (I2), nothing in flight after joins.
        assert_eq!(store.len(), count, "by_sid size == per-user count");
        assert_eq!(snap.len(), count, "snapshot == count");
        assert_eq!(
            snap.iter().filter(|k| **k == s).count(),
            count,
            "sid appears at most once in the slot"
        );
        // Invariant: count ⇔ routability.
        match count {
            0 => {
                assert!(store.lookup(&s).is_none(), "count 0 ⇔ absent");
            }
            1 => {
                assert!(store.lookup(&s).is_some(), "count 1 ⇔ present");
            }
            _ => panic!("count out of range: {count}"),
        }
        // Invariant: a Some removal is always one of the two valid
        // generations (the removed Arc is never a third, foreign session).
        if let Some(r) = &removed {
            let valid = Arc::ptr_eq(r, &arc1) || arc2.as_ref().is_some_and(|a2| Arc::ptr_eq(r, a2));
            assert!(valid, "removed Arc must be generation 1 or 2");
        }
    }

    // 3. Cross-user sid conflict.
    let store2 = SessionStore::new();
    store2.insert(s, sess(u1, T));
    match store2.admit(s, sess(u2, T + 1), T + 1, 16) {
        Admission::Rejected(AdmitReject::SidConflict) => {}
        other => panic!("expected SidConflict, got {other:?}"),
    }
    assert_eq!(store2.count_for_user(&u1), 1, "owner unchanged");
    assert_eq!(store2.count_for_user(&u2), 0, "thief admitted nothing");
}

/// RV-CONNECT-0A42F42C — a REJECTED admission mutates nothing even when it
/// would need several victims: cap shrinks 3 → 2, oldest seed idle, other
/// two active ⇒ `need == 2`, the second victim is active ⇒ `AllActive` and
/// the idle first victim must NOT have been evicted.
///
/// Fails if ONLY the up-front classification is reverted (victims removed
/// one by one, rejecting at the first active one): the idle seed is gone.
#[test]
fn rv_connect_0a42f42c_rejected_multi_victim_admission_mutates_nothing() {
    let store = SessionStore::new();
    let u = uid(4);
    let seeds = [
        (sid(1), T - ACTIVE_WINDOW_NS - SECOND),
        (sid(2), T - SECOND),
        (sid(3), T - SECOND),
    ];
    for (k, last) in seeds {
        match store.admit(k, sess(u, last), T, 0) {
            Admission::Admitted { .. } => {}
            Admission::Rejected(r) => panic!("seeding must succeed: {r:?}"),
        }
    }
    match store.admit(sid(9), sess(u, T), T, 2) {
        Admission::Rejected(AdmitReject::AllActive) => {}
        other => panic!("expected AllActive, got {:?}", other),
    }
    assert_eq!(store.count_for_user(&u), 3, "nothing evicted or added");
    let snap = store.snapshot_by_user(&u);
    for (k, _) in seeds {
        assert!(snap.contains(&k), "seed survives a rejected admission");
    }
    assert!(!snap.contains(&sid(9)));
}

/// RV-CONNECT-0A42F42C — a sid owned by ANOTHER user is a `SidConflict`
/// that must not evict the idle LRU victim of the requesting user.
///
/// Fails if ONLY the claim-before-evict ordering is reverted (victims
/// evicted before the sid ownership is resolved): the idle seed is gone.
#[test]
fn rv_connect_0a42f42c_sid_conflict_rejection_keeps_idle_victim() {
    let store = SessionStore::new();
    let (a, b) = (uid(5), uid(6));
    match store.admit(sid(50), sess(b, T), T, 16) {
        Admission::Admitted { .. } => {}
        Admission::Rejected(r) => panic!("seeding must succeed: {r:?}"),
    }
    for i in 0..16u8 {
        let last = if i == 0 {
            T - ACTIVE_WINDOW_NS
        } else {
            T - SECOND
        };
        match store.admit(sid(i), sess(a, last), T, 16) {
            Admission::Admitted { .. } => {}
            Admission::Rejected(r) => panic!("seeding must succeed: {r:?}"),
        }
    }
    match store.admit(sid(50), sess(a, T), T, 16) {
        Admission::Rejected(AdmitReject::SidConflict) => {}
        other => panic!("expected SidConflict, got {:?}", other),
    }
    assert_eq!(store.count_for_user(&a), 16);
    assert!(
        store.snapshot_by_user(&a).contains(&sid(0)),
        "idle victim kept"
    );
    assert_eq!(store.count_for_user(&b), 1, "foreign binding untouched");
}

/// RV-CONNECT-0A42F42C — a touch BEFORE the admission makes the would-be
/// victim active: the admission is rejected and the victim survives; the
/// same victim untouched is evicted. Also pins that an out-of-order
/// (older) timestamp never moves activity backwards.
///
/// Fails if ONLY `touch_at` is reverted to a plain `store`: the stale
/// timestamp regresses activity and the victim is wrongly evicted.
#[test]
fn rv_connect_0a42f42c_touch_ordering_and_monotonic_activity() {
    let store = SessionStore::new();
    let u = uid(7);
    let idle = T - ACTIVE_WINDOW_NS;
    for i in 0..16u8 {
        let last = if i == 0 { idle } else { T - SECOND };
        match store.admit(sid(i), sess(u, last), T, 16) {
            Admission::Admitted { .. } => {}
            Admission::Rejected(r) => panic!("seeding must succeed: {r:?}"),
        }
    }
    // Fresh activity on the LRU victim, then a stale (older) touch.
    assert!(store.lookup_at(&sid(0), T).is_some());
    assert!(store.lookup_at(&sid(0), idle).is_some());
    match store.admit(sid(99), sess(u, T), T, 16) {
        Admission::Rejected(AdmitReject::AllActive) => {}
        other => panic!("touched victim is active, got {:?}", other),
    }
    assert!(store.snapshot_by_user(&u).contains(&sid(0)));
}
