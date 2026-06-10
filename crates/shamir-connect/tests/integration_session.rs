//! Integration tests for Session + dispatch + per-request validity check (§7.5).

use shamir_connect::common::envelope::{ErrorEnvelope, RequestEnvelope, ResponseEnvelope};
use shamir_connect::common::time::{ns, UnixNanos};
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::session::{Session, SessionPermissions, SessionStore};
use shamir_connect::server::{dispatch_request, DispatchOutcome, RequestHandler};

struct Echo;
impl RequestHandler for Echo {
    fn handle<'a>(
        &'a self,
        _session: &'a shamir_connect::server::Session,
        req: &'a [u8],
    ) -> shamir_connect::server::dispatch::HandlerFuture<'a> {
        let out = req.to_vec();
        Box::pin(async move { Ok(out) })
    }
}

fn make_session(user_id: [u8; 16], created_at_ns: u64) -> Session {
    Session::new(
        user_id,
        "alice".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        created_at_ns,
    )
}

#[test]
fn store_insert_lookup_remove() {
    let store = SessionStore::new();
    let sid = [0xaau8; 32];
    let s = make_session([1u8; 16], 100);
    let _arc = store.insert(sid, s);

    assert_eq!(store.len(), 1);

    let found = store.lookup(&sid).unwrap();
    assert_eq!(found.username, "alice");

    let removed = store.remove(&sid).unwrap();
    assert_eq!(removed.username, "alice");
    assert!(store.is_empty());
}

#[tokio::test]
async fn dispatch_returns_session_expired_for_unknown_sid() {
    let store = SessionStore::new();
    let env = RequestEnvelope::new([0u8; 32], Some(7), b"hello".to_vec());

    let out = dispatch_request(&env, &store, |_| 0, &Echo).await.unwrap();
    match out {
        DispatchOutcome::Error(e) => {
            assert_eq!(e.error, "session_expired");
            assert_eq!(e.request_id, Some(7));
        }
        DispatchOutcome::Response(_) => panic!("expected error"),
    }
}

#[tokio::test]
async fn dispatch_routes_to_handler_for_valid_session() {
    let store = SessionStore::new();
    let sid = [0x55u8; 32];
    let s = make_session([1u8; 16], 100);
    store.insert(sid, s);

    // tickets_invalid_before_ns = 0 → 100 > 0 → valid
    let env = RequestEnvelope::new(sid, Some(42), b"hello world".to_vec());
    let out = dispatch_request(&env, &store, |_| 0, &Echo).await.unwrap();
    match out {
        DispatchOutcome::Response(r) => {
            assert_eq!(r.request_id, Some(42));
            assert_eq!(r.res, b"hello world");
        }
        DispatchOutcome::Error(e) => panic!("unexpected error: {}", e.error),
    }
}

#[tokio::test]
async fn dispatch_kills_session_when_invalidated_per_section_7_5() {
    let store = SessionStore::new();
    let sid = [0x55u8; 32];
    let s = make_session([1u8; 16], 100); // created at T=100
    store.insert(sid, s);

    // Admin sets tickets_invalid_before_ns = 200 (future relative to creation).
    // Per §7.5: created_at_ns(100) <= invalid_before(200) → kick.
    let env = RequestEnvelope::new(sid, None, b"x".to_vec());
    let out = dispatch_request(&env, &store, |_| 200, &Echo)
        .await
        .unwrap();

    match out {
        DispatchOutcome::Error(e) => assert_eq!(e.error, "session_invalidated"),
        _ => panic!("expected session_invalidated"),
    }
    // Store cleaned eagerly so concurrent requests can't reuse.
    assert!(store.lookup(&sid).is_none());
}

#[tokio::test]
async fn dispatch_strict_inequality_at_exact_boundary() {
    let store = SessionStore::new();
    let sid = [0x55u8; 32];
    let s = make_session([1u8; 16], 100);
    store.insert(sid, s);

    // created_at_ns(100) == tickets_invalid_before_ns(100) → strict > → invalid
    let env = RequestEnvelope::new(sid, None, b"x".to_vec());
    let out = dispatch_request(&env, &store, |_| 100, &Echo)
        .await
        .unwrap();
    match out {
        DispatchOutcome::Error(e) => assert_eq!(e.error, "session_invalidated"),
        _ => panic!("strict > must reject equal timestamps"),
    }
}

#[tokio::test]
async fn dispatch_one_nanosecond_after_invalidation_passes() {
    let store = SessionStore::new();
    let sid = [0x55u8; 32];
    let s = make_session([1u8; 16], 101);
    store.insert(sid, s);

    let env = RequestEnvelope::new(sid, None, b"x".to_vec());
    let out = dispatch_request(&env, &store, |_| 100, &Echo)
        .await
        .unwrap();
    assert!(matches!(out, DispatchOutcome::Response(_)));
}

#[test]
fn snapshot_by_user_returns_only_matching() {
    let store = SessionStore::new();
    let alice_uid = [1u8; 16];
    let bob_uid = [2u8; 16];

    store.insert([0xa1u8; 32], make_session(alice_uid, 100));
    store.insert([0xa2u8; 32], make_session(alice_uid, 100));
    store.insert([0xb1u8; 32], make_session(bob_uid, 100));

    let alice_sids = store.snapshot_by_user(&alice_uid);
    assert_eq!(alice_sids.len(), 2);

    let bob_sids = store.snapshot_by_user(&bob_uid);
    assert_eq!(bob_sids.len(), 1);
}

#[test]
fn gc_evicts_idle_sessions() {
    let store = SessionStore::new();
    let now = UnixNanos::now().as_u64();
    let one_hour_ago = now - ns::HOUR;
    store.insert([0x11u8; 32], make_session([1u8; 16], one_hour_ago));

    // idle_ttl = 30 min, max_age = 24h
    let evicted = store.gc_expired(now, 24 * ns::HOUR, 30 * ns::MINUTE);
    assert_eq!(evicted, 1);
    assert_eq!(store.len(), 0);
}

#[test]
fn gc_keeps_active_sessions() {
    let store = SessionStore::new();
    let now = UnixNanos::now().as_u64();
    store.insert([0x11u8; 32], make_session([1u8; 16], now));

    let evicted = store.gc_expired(now + ns::SECOND, 24 * ns::HOUR, 30 * ns::MINUTE);
    assert_eq!(evicted, 0);
    assert_eq!(store.len(), 1);
}

#[test]
fn envelope_round_trip_msgpack() {
    let env = RequestEnvelope::new([0xabu8; 32], Some(123), b"test request".to_vec());
    let bytes = env.to_msgpack().unwrap();
    let decoded = RequestEnvelope::from_msgpack(&bytes).unwrap();
    assert_eq!(env, decoded);
}

#[test]
fn envelope_rejects_wrong_session_id_length() {
    let env = RequestEnvelope {
        session_id: vec![0x01, 0x02, 0x03], // wrong length
        request_id: None,
        req: vec![],
    };
    assert!(env.session_id_array().is_err());
}

#[test]
fn response_envelope_round_trip() {
    let r = ResponseEnvelope::ok(Some(99), b"response body".to_vec());
    let b = r.to_msgpack().unwrap();
    let r2 = ResponseEnvelope::from_msgpack(&b).unwrap();
    assert_eq!(r, r2);
}

#[test]
fn error_envelope_round_trip() {
    let e = ErrorEnvelope::new(Some(99), "session_expired");
    let b = e.to_msgpack().unwrap();
    let e2 = ErrorEnvelope::from_msgpack(&b).unwrap();
    assert_eq!(e, e2);
}

#[test]
fn permissions_snapshot_admin_detection() {
    let p = SessionPermissions::from_roles(vec!["read_write".into(), "superuser".into()]);
    assert!(p.is_superuser);

    let p2 = SessionPermissions::from_roles(vec!["read_only".into()]);
    assert!(!p2.is_superuser);
}

/// Optim #4: dispatch_request_view operates on a borrowed RequestEnvelopeView
/// without copying session_id or req. Functionally identical to dispatch_request.
#[tokio::test]
async fn dispatch_request_view_round_trip() {
    use shamir_connect::common::envelope::RequestEnvelopeView;
    use shamir_connect::server::dispatch::dispatch_request_view;

    let store = SessionStore::new();
    let sid = [0xa1u8; 32];
    store.insert(sid, make_session([0x01u8; 16], UnixNanos::now().as_u64()));

    let env = RequestEnvelope::new(sid, Some(7), b"ping".to_vec());
    let bytes = env.to_msgpack().unwrap();

    let view = RequestEnvelopeView::from_msgpack(&bytes).unwrap();
    assert_eq!(view.session_id_array().unwrap(), &sid);
    assert_eq!(view.request_id, Some(7));
    assert_eq!(view.req, b"ping");

    let outcome = dispatch_request_view(&view, &store, |_| 0u64, &Echo)
        .await
        .unwrap();
    let DispatchOutcome::Response(r) = outcome else {
        panic!("expected response")
    };
    assert_eq!(r.res, b"ping");
    assert_eq!(r.request_id, Some(7));
}

/// Optim #4: dispatch_request_view applies the §7.5 validity check
/// identically to dispatch_request.
#[tokio::test]
async fn dispatch_request_view_kills_session_when_invalidated() {
    use shamir_connect::common::envelope::RequestEnvelopeView;
    use shamir_connect::server::dispatch::dispatch_request_view;

    let store = SessionStore::new();
    let sid = [0xb2u8; 32];
    let uid = [0x02u8; 16];
    let created_at = 1000u64;
    store.insert(sid, make_session(uid, created_at));

    let env = RequestEnvelope::new(sid, Some(99), b"bye".to_vec());
    let bytes = env.to_msgpack().unwrap();
    let view = RequestEnvelopeView::from_msgpack(&bytes).unwrap();

    // tickets_invalid_before_ns >= created_at_ns → §7.5 kicks.
    let outcome = dispatch_request_view(&view, &store, |_| created_at, &Echo)
        .await
        .unwrap();
    let DispatchOutcome::Error(e) = outcome else {
        panic!("expected error")
    };
    assert_eq!(e.error, "session_invalidated");
    assert_eq!(store.len(), 0, "session must be removed");
}

/// Optim #9: `RequestEnvelopeRef<'a>` produces byte-identical msgpack to
/// the owning `RequestEnvelope` AND to a payload that round-trips through
/// `RequestEnvelopeView`. Confirms wire-compat for the zero-copy client
/// encode path.
#[test]
fn request_envelope_ref_wire_compat_with_owning_and_view() {
    use shamir_connect::common::envelope::{RequestEnvelopeRef, RequestEnvelopeView};

    let sid = [0xa1u8; 32];
    let req: &[u8] = b"hello world";

    let owning = RequestEnvelope::new(sid, Some(7), req.to_vec());
    let owning_bytes = owning.to_msgpack().unwrap();

    let borrowed = RequestEnvelopeRef {
        session_id: &sid,
        request_id: Some(7),
        req,
    };
    let borrowed_bytes = borrowed.to_msgpack().unwrap();

    assert_eq!(
        owning_bytes, borrowed_bytes,
        "RequestEnvelopeRef must produce identical bytes to RequestEnvelope"
    );

    // And the view-side decode of either must succeed and observe the
    // expected content.
    let view = RequestEnvelopeView::from_msgpack(&borrowed_bytes).unwrap();
    assert_eq!(view.session_id_array().unwrap(), &sid);
    assert_eq!(view.request_id, Some(7));
    assert_eq!(view.req, req);
}

/// Optim #9: rid omitted on the wire when `None`.
#[test]
fn request_envelope_ref_skips_rid_when_none() {
    use shamir_connect::common::envelope::RequestEnvelopeRef;

    let sid = [0u8; 32];
    let r1 = RequestEnvelopeRef {
        session_id: &sid,
        request_id: None,
        req: b"x",
    };
    let r2 = RequestEnvelope::new(sid, None, b"x".to_vec());
    assert_eq!(r1.to_msgpack().unwrap(), r2.to_msgpack().unwrap());
}

/// v1 #7: `MAX_SESSIONS_PER_USER` LRU eviction — when the user reaches
/// the per-user cap, the LEAST-RECENTLY-USED existing session is evicted
/// (audit reason `max_sessions_lru` per spec §7.4 NORMATIVE).
#[test]
fn insert_with_per_user_cap_evicts_lru_at_threshold() {
    use shamir_connect::server::session::SessionStore;

    let store = SessionStore::new();
    let uid = [0xa1u8; 16];

    // Cap = 3, insert 3 sessions with monotonically-increasing
    // last_activity (so we know which is "oldest").
    let mut sids = Vec::new();
    for i in 0..3 {
        let s = make_session(uid, 1000 + i);
        let sid = [i as u8; 32];
        store.insert(sid, s);
        sids.push(sid);
    }
    assert_eq!(store.len(), 3);

    // 4th insert with cap=3 → must evict the LRU (sids[0], oldest activity).
    let new_sid = [99u8; 32];
    let (_arc, evicted) = store.insert_with_per_user_cap(
        new_sid,
        make_session(uid, 5000),
        3, // cap
    );
    assert_eq!(evicted, Some(sids[0]), "must evict LRU (oldest activity)");
    assert!(store.lookup(&sids[0]).is_none());
    assert!(store.lookup(&sids[1]).is_some());
    assert!(store.lookup(&sids[2]).is_some());
    assert!(store.lookup(&new_sid).is_some());
    assert_eq!(store.len(), 3);
}

/// v1 #7: per-user cap is per-USER, not global. Other users' sessions
/// are not touched.
#[test]
fn insert_with_per_user_cap_does_not_affect_other_users() {
    use shamir_connect::server::session::SessionStore;

    let store = SessionStore::new();
    let alice = [0x11u8; 16];
    let bob = [0x22u8; 16];

    for i in 0..3 {
        store.insert([i as u8; 32], make_session(alice, 1000 + i));
    }
    let bob_sid = [50u8; 32];
    store.insert(bob_sid, make_session(bob, 1000));

    // Alice insert with cap=3 → evicts an alice session, NOT bob's.
    let (_arc, evicted) = store.insert_with_per_user_cap([99u8; 32], make_session(alice, 5000), 3);
    assert!(evicted.is_some());
    assert!(
        store.lookup(&bob_sid).is_some(),
        "bob's session must be untouched"
    );
}

/// v1 #7: cap of 0 disables the LRU policy (insert always succeeds, no
/// eviction).
#[test]
fn insert_with_per_user_cap_zero_disables_eviction() {
    use shamir_connect::server::session::SessionStore;

    let store = SessionStore::new();
    let uid = [0xa1u8; 16];
    for i in 0..50 {
        let (_, evicted) =
            store.insert_with_per_user_cap([i as u8; 32], make_session(uid, 1000 + i), 0);
        assert!(evicted.is_none());
    }
    assert_eq!(store.len(), 50);
}

/// Optim #4: invalid session_id length is rejected.
#[test]
fn request_envelope_view_rejects_wrong_session_id_length() {
    use serde::Serialize;
    use shamir_connect::common::envelope::RequestEnvelopeView;

    #[derive(Serialize)]
    struct Bad {
        #[serde(with = "serde_bytes", rename = "sid")]
        session_id: Vec<u8>,
        #[serde(rename = "rid", skip_serializing_if = "Option::is_none")]
        request_id: Option<u32>,
        #[serde(with = "serde_bytes")]
        req: Vec<u8>,
    }
    let bad = Bad {
        session_id: vec![0u8; 16], // too short
        request_id: None,
        req: vec![],
    };
    let bytes = rmp_serde::to_vec_named(&bad).unwrap();
    let view = RequestEnvelopeView::from_msgpack(&bytes).unwrap();
    assert!(view.session_id_array().is_err());
}
