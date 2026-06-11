//! Integration tests for the duplex request loop (M1).
//!
//! Drives `request_loop` directly via `TcpFramer` over `tokio::io::duplex`,
//! using a stub `RequestHandler` that can simulate slow and fast responses.
//!
//! # Test layout
//!
//! * **Test 1** — overtake: a slow request (rid=1) is sent first, then a fast
//!   one (rid=2); with cap > 1 the fast reply arrives first.
//! * **Test 2** — lock-step (`max_in_flight = 1`): replies arrive in wire order
//!   regardless of handler speed.
//! * **Test 3** — `ReplyAndBreak`: a missing session triggers `session_expired`;
//!   the reply arrives and then the connection is closed (EOF on next read).
//! * **Test 4** — burst: 16 concurrent requests all get responses; rid set matches.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::duplex;
use tokio::time::timeout;

use shamir_connect::common::envelope::{ErrorEnvelope, RequestEnvelope, ResponseEnvelope};
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::{HandlerFuture, RequestHandler};
use shamir_connect::server::session::{Session, SessionPermissions, SessionStore};

use shamir_server::connection::request_loop;
use shamir_server::framer::{Framer, TcpFramer};

use shamir_transport_tcp::framing::{read_frame_into, write_frame_into, MAX_FRAME_SIZE_DEFAULT};

// ---------------------------------------------------------------------------
// Stub handler
// ---------------------------------------------------------------------------

/// A minimal `RequestHandler` for testing.
///
/// * `"slow"` body → sleep 200 ms then echo `"slow_ok"`.
/// * `"fast"` body → echo `"fast_ok"` immediately.
/// * Everything else → echo unchanged.
struct StubHandler;

impl RequestHandler for StubHandler {
    fn handle<'a>(
        &'a self,
        _session: &'a Session,
        req: &'a [u8],
        _conn: &'a ConnectionServices,
    ) -> HandlerFuture<'a> {
        Box::pin(async move {
            match req {
                b"slow" => {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    Ok(b"slow_ok".to_vec())
                }
                b"fast" => Ok(b"fast_ok".to_vec()),
                other => Ok(other.to_vec()),
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a minimal `Arc<ConnectionContext>` with the given `max_in_flight` and
/// a pre-inserted session with `session_id = [0xAB; 32]`.
fn make_ctx_with_session(
    max_in_flight: usize,
) -> (Arc<shamir_server::connection::ConnectionContext>, [u8; 32]) {
    use shamir_connect::common::kdf_params::KdfParams;
    use shamir_connect::server::argon2_semaphore::Argon2Semaphore;
    use shamir_connect::server::audit_chain::{AuditChain, AuditChainWriter};
    use shamir_connect::server::config::ServerSecrets;
    use shamir_connect::server::lockout::InMemoryLockoutStore;
    use shamir_connect::server::rate_limit::InMemoryRateLimiter;
    use shamir_connect::server::resume::ResumeConfig;
    use shamir_connect::server::rotation::ServerIdentityState;
    let kdf = KdfParams {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    };

    let seed = [0x11u8; 32];
    let identity = ServerIdentityState::from_material(&seed, None, None, 1);
    let keypair = shamir_connect::common::crypto::Ed25519Keypair::from_seed(&seed);

    let secrets = Arc::new(ServerSecrets {
        server_secret: [0x22u8; 32],
        lockout_secret: [0x33u8; 32],
    });

    let session_store = Arc::new(SessionStore::new());

    // Insert a session so dispatches can find it.
    let session_id = [0xABu8; 32];
    let session = Session::new(
        [0u8; 16],
        "test".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        1_000_000,
    );
    session_store.insert(session_id, session);

    // User directory in a temp file (TempDir kept alive via Box::leak for test).
    let temp = tempfile::TempDir::new().unwrap();
    let temp_path = temp.path().join("u.redb");
    let user_dir = shamir_server::user_directory::RedbUserDirectory::open(&temp_path)
        .expect("open user dir for test");
    // Leak the temp dir so the redb file lives until process exit (test only).
    Box::leak(Box::new(temp));

    // No-op audit: in-memory chain + noop appender.
    let chain = AuditChain::new([0u8; 32]);
    struct NoopAppender;
    impl shamir_connect::server::audit_chain::AuditAppender for NoopAppender {
        fn append_entry(&self, _entry: &shamir_connect::server::audit_chain::AuditEntry) {}
        fn checkpoint(&self, _next_seq: u64, _prev_hmac: &[u8; 32]) {}
    }
    let appender =
        Arc::new(NoopAppender) as Arc<dyn shamir_connect::server::audit_chain::AuditAppender>;
    let audit = Arc::new(AuditChainWriter::new(chain, appender));

    let resume = Arc::new(ResumeConfig::new([0x44u8; 32], None, false, false));

    let counters = Arc::new(shamir_connect::server::resume::InMemoryConsumedCounters::new());
    let ctx = shamir_server::connection::ConnectionContext::new(
        Arc::new(identity),
        keypair,
        secrets,
        kdf,
        session_store,
        Arc::new(user_dir),
        Arc::new(InMemoryLockoutStore::new()),
        Arc::new(InMemoryRateLimiter::new(0)),
        Arc::new(Argon2Semaphore::with_capacity(4)),
        audit,
        resume,
        counters,
        Arc::new(StubHandler),
        BindingMode::TlsExporter,
        TransportKind::Tcp,
        None,
        Duration::from_secs(5),
        max_in_flight,
    );

    (ctx, session_id)
}

/// Write one `RequestEnvelope` onto `writer`.
async fn client_send<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    sid: &[u8; 32],
    rid: u32,
    body: &[u8],
) {
    let env = RequestEnvelope {
        session_id: sid.to_vec(),
        request_id: Some(rid),
        req: body.to_vec(),
    };
    let bytes = env.to_msgpack().unwrap();
    let mut scratch = Vec::new();
    write_frame_into(writer, &bytes, &mut scratch)
        .await
        .unwrap();
}

/// Read one `ResponseEnvelope` and return its `request_id`.
async fn client_recv_rid<R: tokio::io::AsyncRead + Unpin>(reader: &mut R) -> u32 {
    let mut buf = Vec::new();
    read_frame_into(reader, MAX_FRAME_SIZE_DEFAULT, &mut buf)
        .await
        .unwrap();
    let resp = ResponseEnvelope::from_msgpack(&buf).unwrap();
    resp.request_id.expect("response must carry a request_id")
}

/// Read one `ErrorEnvelope` and return the error string.
async fn client_recv_error<R: tokio::io::AsyncRead + Unpin>(reader: &mut R) -> String {
    let mut buf = Vec::new();
    read_frame_into(reader, MAX_FRAME_SIZE_DEFAULT, &mut buf)
        .await
        .unwrap();
    // May be ResponseEnvelope or ErrorEnvelope; try error first.
    if let Ok(err) = ErrorEnvelope::from_msgpack(&buf) {
        return err.error;
    }
    panic!("expected ErrorEnvelope but got something else");
}

// ---------------------------------------------------------------------------
// Test 1 — overtake: fast response arrives before slow with cap > 1
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn duplex_test1_fast_overtakes_slow() {
    let (ctx, sid) = make_ctx_with_session(8);

    let (server_io, client_io) = duplex(128 * 1024);
    let (server_r, server_w) = TcpFramer::new(server_io).split();

    let loop_task = tokio::spawn(request_loop(ctx, server_r, server_w, sid));

    let (mut cr, mut cw) = tokio::io::split(client_io);

    // Send slow (rid=1) then fast (rid=2) without waiting.
    client_send(&mut cw, &sid, 1, b"slow").await;
    client_send(&mut cw, &sid, 2, b"fast").await;

    // With cap=8, both dispatch tasks run concurrently.
    // fast (200 ms sleep vs 0 ms) should complete first.
    let first = timeout(Duration::from_secs(2), client_recv_rid(&mut cr))
        .await
        .expect("first reply timed out");
    let second = timeout(Duration::from_secs(2), client_recv_rid(&mut cr))
        .await
        .expect("second reply timed out");

    assert_eq!(first, 2, "fast request (rid=2) should arrive first");
    assert_eq!(second, 1, "slow request (rid=1) should arrive second");

    // Drop both halves so the underlying DuplexStream is closed; this
    // signals EOF to the server's reader and allows request_loop to exit.
    drop(cw);
    drop(cr);
    let _ = loop_task.await;
}

// ---------------------------------------------------------------------------
// Test 2 — lock-step: max_in_flight=1 forces ordered replies
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn duplex_test2_cap1_lockstep_order() {
    let (ctx, sid) = make_ctx_with_session(1);

    let (server_io, client_io) = duplex(128 * 1024);
    let (server_r, server_w) = TcpFramer::new(server_io).split();

    let loop_task = tokio::spawn(request_loop(ctx, server_r, server_w, sid));

    let (mut cr, mut cw) = tokio::io::split(client_io);

    // Send slow (rid=1) then fast (rid=2).
    // With cap=1 the reader stalls after accepting rid=1 until its permit
    // is released — so rid=2 is dispatched only after rid=1 completes.
    client_send(&mut cw, &sid, 1, b"slow").await;
    client_send(&mut cw, &sid, 2, b"fast").await;

    let first = timeout(Duration::from_secs(3), client_recv_rid(&mut cr))
        .await
        .expect("first reply timed out");
    let second = timeout(Duration::from_secs(3), client_recv_rid(&mut cr))
        .await
        .expect("second reply timed out");

    assert_eq!(first, 1, "cap=1: first reply must be rid=1 (slow)");
    assert_eq!(second, 2, "cap=1: second reply must be rid=2 (fast)");

    drop(cw);
    drop(cr);
    let _ = loop_task.await;
}

// ---------------------------------------------------------------------------
// Test 3 — ReplyAndBreak: unknown session → reply + connection close
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn duplex_test3_reply_and_close_on_session_expired() {
    // Use a context with a session_id that is NOT inserted in the store.
    let (ctx, _valid_sid) = make_ctx_with_session(8);
    let unknown_sid = [0xFFu8; 32]; // deliberately absent from the store

    let (server_io, client_io) = duplex(128 * 1024);
    let (server_r, server_w) = TcpFramer::new(server_io).split();

    // Pass the unknown sid — the loop will still process the request and send
    // session_expired before closing.
    let loop_task = tokio::spawn(request_loop(ctx, server_r, server_w, unknown_sid));

    let (mut cr, mut cw) = tokio::io::split(client_io);

    client_send(&mut cw, &unknown_sid, 42, b"anything").await;

    let err = timeout(Duration::from_secs(2), client_recv_error(&mut cr))
        .await
        .expect("error reply timed out");
    assert_eq!(err, "session_expired");

    // Server must close the connection; next read returns an error.
    let mut buf = Vec::new();
    let result = timeout(
        Duration::from_secs(2),
        read_frame_into(&mut cr, MAX_FRAME_SIZE_DEFAULT, &mut buf),
    )
    .await
    .expect("close signal timed out");
    assert!(
        result.is_err(),
        "connection must be closed after session_expired response"
    );

    // Drop both halves so the underlying DuplexStream closes; this allows
    // the server's blocking read_frame_into to see EOF and exit.
    drop(cr);
    drop(cw);
    let _ = loop_task.await;
}

// ---------------------------------------------------------------------------
// Test 5 — Defect B regression: server exits even when client holds the
// TCP half open after receiving session_expired (ReplyAndClose path).
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn duplex_test5_server_exits_after_reply_and_close_without_client_eof() {
    // Context with NO inserted session — every request will yield session_expired.
    let (ctx, _valid_sid) = make_ctx_with_session(8);
    let unknown_sid = [0xCCu8; 32];

    let (server_io, client_io) = duplex(128 * 1024);
    let (server_r, server_w) = TcpFramer::new(server_io).split();

    let loop_task = tokio::spawn(request_loop(ctx, server_r, server_w, unknown_sid));

    let (mut cr, mut cw) = tokio::io::split(client_io);

    // Send one request with the unknown sid.
    client_send(&mut cw, &unknown_sid, 1, b"anything").await;

    // Read the session_expired error reply.
    let err = timeout(Duration::from_secs(2), client_recv_error(&mut cr))
        .await
        .expect("error reply timed out");
    assert_eq!(err, "session_expired");

    // Intentionally do NOT close `cw` or `cr` — the client holds the
    // connection open. The server must still terminate on its own because the
    // writer task exited after ReplyAndClose, and the reader's select! branch
    // catches that (Defect B fix).
    let server_result = timeout(Duration::from_secs(2), loop_task).await;
    assert!(
        server_result.is_ok(),
        "request_loop must exit within 2 s after ReplyAndClose even if client holds the socket open"
    );

    // Cleanup — drop client halves now that the test assertion passed.
    drop(cw);
    drop(cr);
}

// ---------------------------------------------------------------------------
// Test 4 — burst: 16 concurrent requests all get responses
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn duplex_test4_burst_16_requests() {
    const N: u32 = 16;
    let (ctx, sid) = make_ctx_with_session(N as usize);

    let (server_io, client_io) = duplex(512 * 1024);
    let (server_r, server_w) = TcpFramer::new(server_io).split();

    let loop_task = tokio::spawn(request_loop(ctx, server_r, server_w, sid));

    let (mut cr, mut cw) = tokio::io::split(client_io);

    // Send all 16 requests without waiting.
    for rid in 1..=N {
        client_send(&mut cw, &sid, rid, b"fast").await;
    }

    // Collect all 16 responses.
    let mut received_rids = std::collections::HashSet::new();
    for _ in 0..N {
        let rid = timeout(Duration::from_secs(2), client_recv_rid(&mut cr))
            .await
            .expect("burst reply timed out");
        received_rids.insert(rid);
    }

    let expected: std::collections::HashSet<u32> = (1..=N).collect();
    assert_eq!(
        received_rids, expected,
        "all 16 rids must be received exactly once"
    );

    drop(cw);
    drop(cr);
    let _ = loop_task.await;
}
