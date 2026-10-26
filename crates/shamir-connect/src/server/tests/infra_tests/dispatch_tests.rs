//! Tests for the owned `dispatch_request` path (server/infra/dispatch.rs).

use crate::common::envelope::{RequestEnvelope, RequestEnvelopeView};
use crate::common::time::UnixNanos;
use crate::common::types::{BindingMode, TransportKind};
use crate::server::conn_services::ConnectionServices;
use crate::server::dispatch::dispatch_request_view;
use crate::server::dispatch::HandlerFuture;
use crate::server::session::{Session, SessionPermissions, SessionStore};
use crate::server::{dispatch_request, DispatchOutcome, RequestHandler};

struct Echo;
impl RequestHandler for Echo {
    fn handle<'a>(
        &'a self,
        _session: &'a Session,
        req: &'a [u8],
        _conn: &'a ConnectionServices,
    ) -> HandlerFuture<'a> {
        let out = req.to_vec();
        Box::pin(async move { Ok(out) })
    }
}

/// Handler that fails with an arbitrary diagnostic string.
struct Failing;
impl RequestHandler for Failing {
    fn handle<'a>(
        &'a self,
        _session: &'a Session,
        _req: &'a [u8],
        _conn: &'a ConnectionServices,
    ) -> HandlerFuture<'a> {
        Box::pin(async move { Err("encode_error: leaked secret path /tmp/x [1,2,3]".to_string()) })
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

/// RV-CONNECT-8AD46452: the owned dispatch path must enforce the same
/// post-auth per-session rate limit as the borrowed-view path. Pre-fix,
/// a depleted bucket still reached the handler; post-fix, the request is
/// rejected with `rate_limited` before the handler runs.
#[tokio::test]
async fn rv_dispatch_8ad46452_owned_path_enforces_post_auth_rate_limit() {
    let store = SessionStore::new();
    let sid = [0x77u8; 32];
    let session = make_session([1u8; 16], UnixNanos::now().as_u64());
    store.insert(sid, session);

    // Drain the bucket with timestamps pinned at `u64::MAX`: the first
    // `check` fetch_maxes the refill watermark to MAX (elapsed saturates,
    // refill clamped to capacity); every later check — including
    // dispatch's real-clock call, since `real_now.saturating_sub(MAX) == 0`
    // — computes elapsed 0 and refills nothing, so no wall-clock advance
    // can resurrect a token.
    let live = store.lookup(&sid).expect("session inserted");
    let now_ns = u64::MAX;
    for _ in 0..shamir_tunables::instance_defaults::POST_AUTH_RATE_LIMIT_PER_SEC {
        assert!(live.check_post_auth_rate_limit(now_ns).is_none());
    }
    assert!(live.check_post_auth_rate_limit(now_ns).is_some());

    // Owned dispatch must gate on the same depleted bucket.
    let env = RequestEnvelope::new(sid, Some(9), b"hello".to_vec());
    let out = dispatch_request(
        &env,
        &store,
        |_| 0,
        &Echo,
        &ConnectionServices::without_push(0),
    )
    .await
    .unwrap();

    match out {
        DispatchOutcome::Error(e) => {
            assert_eq!(e.error, "rate_limited");
            assert_eq!(e.request_id, Some(9));
        }
        DispatchOutcome::Response(_) => {
            panic!("owned dispatch must enforce the post-auth rate limit")
        }
    }
}

/// RV-CONNECT-2A423D73: handler `Err(String)` diagnostics must never reach
/// the wire verbatim — both dispatch paths collapse them to the fixed
/// `application_error` code. Pre-fix, `err.error` was the raw handler
/// string (including any embedded payload/path fragments).
#[tokio::test]
async fn rv_connect_2a423d73_handler_err_collapses_to_application_error() {
    let store = SessionStore::new();
    let sid = [0x78u8; 32];
    store.insert(sid, make_session([2u8; 16], UnixNanos::now().as_u64()));
    let env = RequestEnvelope::new(sid, Some(3), b"q".to_vec());

    let out = dispatch_request(
        &env,
        &store,
        |_| 0,
        &Failing,
        &ConnectionServices::without_push(0),
    )
    .await
    .unwrap();

    match out {
        DispatchOutcome::Error(e) => {
            assert_eq!(e.error, "application_error");
            assert_eq!(e.request_id, Some(3));
        }
        DispatchOutcome::Response(_) => {
            panic!("failing handler must produce an error envelope")
        }
    }

    // Same boundary on the borrowed-view path.
    let wire = env.to_msgpack().unwrap();
    let view = RequestEnvelopeView::from_msgpack(&wire).unwrap();
    let out = dispatch_request_view(
        &view,
        &store,
        |_| 0,
        &Failing,
        &ConnectionServices::without_push(0),
    )
    .await
    .unwrap();
    match out {
        DispatchOutcome::Error(e) => assert_eq!(e.error, "application_error"),
        DispatchOutcome::Response(_) => panic!("view path must collapse handler errors too"),
    }
}
