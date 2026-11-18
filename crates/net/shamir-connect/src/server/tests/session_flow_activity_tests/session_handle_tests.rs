//! R4b-1 tests for the minted non-secret session handle and the O(1)
//! activity mirror ([`SessionStore::admit`] stamping, `Session::
//! activity_handle`, `SessionStore::len`, `SessionStore::
//! activity_snapshot`).

use crate::server::admission::{Admission, ACTIVE_WINDOW_NS};
use crate::server::auth::token::Verified;
use crate::server::session::{Session, SessionPermissions, SessionStore};
use crate::server::state::token_session::TokenSocketCaps;
use crate::server::token::{TokenChannels, TokenMode, TokenRecord, TokenState};
use crate::server::TokenSessionStore;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::thread;

const T: u64 = 1_000_000_000;

fn sess(user: [u8; 16], last_activity_ns: u64) -> Session {
    Session::new(
        user,
        "u".to_string(),
        SessionPermissions::from_roles(vec![]),
        crate::common::types::TransportKind::WebSocket,
        crate::common::types::BindingMode::None,
        [0u8; 32],
        last_activity_ns,
    )
}

fn uid(n: u8) -> [u8; 16] {
    [n; 16]
}

fn sid(bytes: [u8; 32]) -> [u8; 32] {
    bytes
}

/// Independent count of the primary registry, via the public `for_each`
/// iterator (never the mirror under test).
fn registry_count(store: &SessionStore) -> usize {
    let mut n = 0usize;
    store.for_each(|_, _| n += 1);
    n
}

fn expect_admitted(
    store: &SessionStore,
    s: [u8; 32],
    user: [u8; 16],
    now: u64,
    cap: usize,
) -> Arc<Session> {
    match store.admit(s, sess(user, now), now, cap) {
        Admission::Admitted { session, .. } => session,
        Admission::Rejected(reason) => panic!("unexpected rejection: {reason:?}"),
    }
}

/// Freshly minted handles are unique and strictly monotonic (counter starts
/// at 1; 0 is the internal un-stamped sentinel and is never published).
#[test]
fn activity_handle_unique_and_monotonic() {
    let store = SessionStore::new();
    let mut prev = 0u64;
    let mut seen = BTreeSet::new();
    for i in 0..64u8 {
        let s = sid([i; 32]);
        let session = expect_admitted(&store, s, uid(1), T, 0);
        let h = session.activity_handle();
        assert_ne!(h, 0, "handle 0 is the un-stamped sentinel");
        assert!(h > prev, "handle {h} must exceed the previous {prev}");
        assert!(seen.insert(h), "duplicate handle {h}");
        prev = h;
    }
    assert_eq!(seen.len(), 64);
}

/// A same-sid, same-user re-admission is a REPLACEMENT (same bearer = same
/// session under the current auth semantics: `resume.rs` mints a fresh
/// random sid on every resume, so sid reuse can only be the same bearer
/// generation being re-admitted). The replacement inherits the handle.
#[test]
fn activity_handle_preserved_across_same_bearer_replacement() {
    let store = SessionStore::new();
    let s = sid([9; 32]);
    let first = expect_admitted(&store, s, uid(1), T, 16);
    let h1 = first.activity_handle();
    let second = expect_admitted(&store, s, uid(1), T + 1, 16);
    assert_eq!(second.activity_handle(), h1, "replacement inherits handle");
    assert_eq!(store.len(), 1, "replacement does not change the count");
}

/// Mirror accounting across every `by_sid` mutation kind: admit (+1),
/// failed admit (SidConflict, no change), same-sid replacement (no change),
/// eviction to make room (net +1 per survivor set), `remove` (-1),
/// `gc_expired` (-1 per removed generation).
#[test]
fn activity_len_mirror_tracks_all_mutations() {
    let store = SessionStore::new();

    // +1 per admit.
    for i in 0..4u8 {
        expect_admitted(&store, sid([i; 32]), uid(1), T, 16);
    }
    assert_eq!(store.len(), 4);
    assert_eq!(registry_count(&store), 4);

    // SidConflict rejection: no mirror change.
    let conflict = store.admit(sid([0; 32]), sess(uid(2), T), T, 16);
    assert!(matches!(conflict, Admission::Rejected(_)));
    assert_eq!(store.len(), 4);

    // Replacement: no mirror change.
    expect_admitted(&store, sid([0; 32]), uid(1), T + 1, 16);
    assert_eq!(store.len(), 4);

    // remove(): -1.
    let removed = store.remove(&sid([3; 32]));
    assert!(removed.is_some());
    assert_eq!(store.len(), 3);

    // remove() of an absent sid: no change.
    assert!(store.remove(&sid([250; 32])).is_none());
    assert_eq!(store.len(), 3);

    // gc_expired(): -1 per removed generation (idle beyond ACTIVE_WINDOW_NS).
    // The replacement at T+1 has last_activity_ns = T+1, and `is_expired` is
    // strict `>`, so GC `now` must be at least T + 1 + ACTIVE_WINDOW_NS + 1
    // to evict it too; T + ACTIVE_WINDOW_NS + 2 keeps max_age bounded while
    // clearing every surviving generation.
    // max_age is deliberately not u64::MAX: `Session::is_expired` computes
    // plain `created_at_ns + max_age_ns`, which would overflow-panic here
    // (created_at_ns = T). u64::MAX / 2 is far beyond any realistic age, so
    // expiry below is driven solely by the idle TTL, as intended.
    let gone = store.gc_expired(T + ACTIVE_WINDOW_NS + 2, u64::MAX / 2, ACTIVE_WINDOW_NS);
    assert_eq!(gone, 3);
    assert_eq!(store.len(), 0);
    assert_eq!(registry_count(&store), 0);
}

/// Eviction inside `admit` (cap pressure, idle LRU victims) keeps the
/// mirror exact: victims -1, admitted +1.
#[test]
fn activity_len_mirror_tracks_admit_eviction() {
    let store = SessionStore::new();
    // 4 idle sessions at T (age >= ACTIVE_WINDOW_NS at now = T + WINDOW).
    for i in 0..4u8 {
        expect_admitted(&store, sid([i; 32]), uid(1), T, 16);
    }
    assert_eq!(store.len(), 4);
    let now = T + ACTIVE_WINDOW_NS;
    match store.admit(sid([200; 32]), sess(uid(1), now), now, 4) {
        Admission::Admitted {
            session,
            evicted,
            replaced,
        } => {
            assert!(!evicted.is_empty(), "cap 4 over 4 idle sessions evicts");
            assert!(!replaced, "new sid, nothing replaced");
            let _ = session.activity_handle();
        }
        Admission::Rejected(reason) => panic!("unexpected rejection: {reason:?}"),
    }
    assert_eq!(store.len(), 4);
    assert_eq!(registry_count(&store), 4);
}

/// Concurrent admits of distinct sids: every published generation carries a
/// unique stamped handle and the mirror equals the true entry count.
#[test]
fn activity_handle_concurrent_unique_and_mirror_exact() {
    const N: usize = 32;
    let store = Arc::new(SessionStore::new());
    let handles: Vec<_> = (0..N)
        .map(|i| {
            let store = Arc::clone(&store);
            thread::spawn(move || {
                let mut b = [0u8; 32];
                b[0] = i as u8;
                b[1] = 0xAA;
                let session = expect_admitted(&store, b, uid(1), T, 0);
                session.activity_handle()
            })
        })
        .collect();
    let minted: Vec<u64> = handles
        .into_iter()
        .map(|h| h.join().expect("admit thread must not panic"))
        .collect();
    let unique: BTreeSet<u64> = minted.iter().copied().collect();
    assert_eq!(unique.len(), N, "all concurrent handles must be unique");
    assert!(minted.iter().all(|h| *h != 0));
    assert_eq!(store.len(), N);
    assert_eq!(registry_count(&store), N);
}

/// `activity_snapshot` samples the primary registry: one Arc per live
/// generation, all stamped, shrinking with removals, without any lock
/// exposure to callers (returns owned Arcs).
#[test]
fn activity_snapshot_matches_registry() {
    let store = SessionStore::new();
    for i in 0..5u8 {
        expect_admitted(&store, sid([i; 32]), uid(1), T, 0);
    }
    let snap = store.activity_snapshot();
    assert_eq!(snap.len(), 5);
    let hs: BTreeSet<u64> = snap.iter().map(|s| s.activity_handle()).collect();
    assert_eq!(hs.len(), 5, "snapshot handles must be unique and stamped");

    store.remove(&sid([1; 32]));
    let snap = store.activity_snapshot();
    assert_eq!(snap.len(), 4);
    assert!(!snap.iter().any(|s| s.session_id == sid([1; 32])));
}

/// Replacement + removal converge the ArcSwap projection: after quiescence
/// `activity_snapshot` holds exactly the live generations with their
/// inherited (replacement) or removed handles — no resurrection, no
/// stranding, gauge-balanced with `len()`.
#[test]
fn activity_snapshot_tracks_replacement_and_removal() {
    let store = SessionStore::new();
    let s = sid([7; 32]);
    let first = expect_admitted(&store, s, uid(1), T, 16);
    let h1 = first.activity_handle();
    let second = expect_admitted(&store, s, uid(1), T + 1, 16);
    assert_eq!(second.activity_handle(), h1);
    let snap = store.activity_snapshot();
    assert_eq!(snap.len(), 1);
    assert!(
        Arc::ptr_eq(&snap[0], &second),
        "projection holds the replacement"
    );
    assert!(store.remove(&s).is_some());
    assert!(store.activity_snapshot().is_empty());
    assert_eq!(store.len(), 0);
    assert_eq!(registry_count(&store), 0);
}

fn token_session(id: u8, owner: [u8; 16]) -> Session {
    let record = TokenRecord {
        id: [id; 16],
        owner_user_id: owner,
        label: "label".into(),
        mac: [0; 32],
        key_id: 1,
        format_version: 1,
        created_at_ns: 1,
        expires_at_ns: 100,
        mode: TokenMode::ReadOnly,
        channels: TokenChannels::Ws,
        databases: None,
        roles: vec![],
        revoked_at_ns: None,
    };
    Session::for_token(
        &Verified {
            state: Arc::new(TokenState::new(record)),
        },
        "u".into(),
        2,
        [9; 32],
    )
}

/// Unified non-secret wire handles: token sessions mint from the same
/// process-wide counter as `SessionStore`, so every stored session across
/// both stores has a unique nonzero handle; the token store's projection is
/// gauge-balanced (`len()`) and lock-free readable via `activity_snapshot`.
#[test]
fn token_store_handles_unified_and_projection_balanced() {
    let scram = SessionStore::new();
    let tokens = TokenSessionStore::new();
    let mut all = BTreeSet::new();
    let scram_session = expect_admitted(&scram, sid([11; 32]), uid(1), T, 0);
    assert!(all.insert(scram_session.activity_handle()));
    let caps = TokenSocketCaps {
        per_token: 8,
        per_owner: 8,
    };
    for i in 0..3u8 {
        let session = tokens
            .admit([100 + i; 32], token_session(1, uid(2)), caps)
            .expect("caps permit 3 sessions");
        assert_ne!(session.activity_handle(), 0);
        assert!(all.insert(session.activity_handle()), "duplicate handle");
    }
    assert_eq!(tokens.len(), 3);
    assert_eq!(tokens.activity_snapshot().len(), 3);
    assert!(tokens.remove(&[100; 32]));
    assert!(!tokens.remove(&[100; 32]));
    assert_eq!(tokens.len(), 2);
    assert_eq!(tokens.activity_snapshot().len(), 2);
    assert!(!tokens
        .activity_snapshot()
        .iter()
        .any(|s| s.session_id == [100; 32]));
    assert_eq!(all.len(), 4, "SCRAM + token handles all unique");
}
