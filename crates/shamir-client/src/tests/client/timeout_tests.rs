//! Connect/request timeout tests (task #520).
//!
//! These prove that the two new [`crate::ConnectOptions`] knobs actually
//! *fire* (not merely compile): a bounded `connect_timeout` gives up on an
//! unresponsive endpoint, a bounded `request_timeout` gives up on a server
//! that accepts the connection but never answers a request — and that in
//! both cases `None` preserves the original *unbounded-wait* behaviour.
//!
//! We exercise the exact timeout wrappers directly (`connect_tcp` and
//! `await_pending_response`) so the tests are deterministic and independent
//! of a live server: the request-side test drives a `oneshot` that the
//! "server" (the test) simply never resolves.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::oneshot;
use zeroize::Zeroizing;

use crate::client::{
    await_pending_response, connect_tcp, ConnectOptions, Lifecycle, LifecycleHooks,
};
use crate::error::ClientError;
use crate::Client;

/// A guaranteed-unroutable private black-hole address: TCP `connect` to it
/// neither completes nor is refused quickly (the SYN is dropped), so a
/// bounded `connect_timeout` is the only thing that unblocks the caller.
const BLACKHOLE: &str = "10.255.255.1:9";

fn make_state() -> Lifecycle {
    Lifecycle::new()
}

// ─── connect_timeout ─────────────────────────────────────────────────────────

/// `connect_timeout = Some(d)` must give up on an unresponsive endpoint with
/// `ConnectTimeout`, roughly after `d` — not instantly, not hanging forever.
#[tokio::test]
async fn connect_timeout_fires() {
    let addr = BLACKHOLE.parse().expect("parse blackhole addr");
    let budget = Duration::from_millis(250);

    let start = Instant::now();
    let res = connect_tcp(addr, Some(budget)).await;
    let elapsed = start.elapsed();

    assert!(
        matches!(res, Err(ClientError::ConnectTimeout(d)) if d == budget),
        "expected ConnectTimeout({budget:?}), got {res:?}"
    );
    // It must have actually waited on the timer, not returned instantly.
    assert!(
        elapsed >= budget,
        "connect returned too early ({elapsed:?} < {budget:?}) — timer did not drive the timeout"
    );
    // And it must not have hung far past the deadline.
    assert!(
        elapsed < budget + Duration::from_secs(5),
        "connect took far too long ({elapsed:?}) — timeout did not fire"
    );
}

/// `connect_timeout = None` must preserve the *unbounded-wait* behaviour:
/// the call does NOT short-circuit with a client-side timeout error. We prove
/// this by racing the (deliberately-hanging) black-hole connect against a
/// short test-side timer and asserting the TEST timer wins — i.e. the client
/// was still waiting.
#[tokio::test]
async fn connect_none_preserves_unbounded_wait() {
    let addr: std::net::SocketAddr = BLACKHOLE.parse().expect("parse blackhole addr");

    let raced = tokio::time::timeout(Duration::from_millis(300), connect_tcp(addr, None)).await;

    // The test-side timer elapsed first → `connect_tcp(None)` was still
    // pending (unbounded). If the client had its own timeout, `raced` would
    // be `Ok(Err(ConnectTimeout))` instead.
    assert!(
        raced.is_err(),
        "connect_tcp(None) resolved on its own ({raced:?}) — None did NOT preserve unbounded wait"
    );
}

// ─── request_timeout ─────────────────────────────────────────────────────────

/// `request_timeout = Some(d)` must give up on a never-answered request with
/// `RequestTimeout`, roughly after `d`, and must clean up the orphaned
/// pending entry so the map does not leak a dead sender.
#[tokio::test]
async fn request_timeout_fires_and_cleans_pending() {
    let state = make_state();
    let rid: u32 = 7;
    // Register a waiter but NEVER send on `tx` (simulate a server that
    // accepts the connection but never answers this rid). We keep `tx` alive
    // in the map, otherwise the receiver would resolve with
    // `ConnectionClosed` instead of timing out.
    let (tx, rx) = oneshot::channel::<Result<Vec<u8>, ClientError>>();
    state.pending.lock().unwrap().map.insert(rid, tx);

    let budget = Duration::from_millis(200);
    let start = Instant::now();
    let res = await_pending_response(rx, Some(budget), &state, rid).await;
    let elapsed = start.elapsed();

    assert!(
        matches!(res, Err(ClientError::RequestTimeout(d)) if d == budget),
        "expected RequestTimeout({budget:?}), got {res:?}"
    );
    assert!(
        elapsed >= budget,
        "request returned too early ({elapsed:?} < {budget:?}) — timer did not drive the timeout"
    );
    // The orphaned pending entry must have been removed on timeout.
    assert!(
        !state.pending.lock().unwrap().map.contains_key(&rid),
        "timed-out request left a dead sender in the pending map"
    );
}

/// `request_timeout = None` must preserve the *unbounded-wait* behaviour:
/// the await does NOT resolve on its own while no response arrives. We prove
/// it by racing the never-answered await against a short test-side timer and
/// asserting the TEST timer wins; then, delivering a response resolves the
/// same future with the payload (the response path is unaffected).
#[tokio::test]
async fn request_none_preserves_unbounded_wait_then_delivers() {
    let state = make_state();
    let rid: u32 = 3;
    let (tx, rx) = oneshot::channel::<Result<Vec<u8>, ClientError>>();
    state.pending.lock().unwrap().map.insert(rid, tx);

    let fut = await_pending_response(rx, None, &state, rid);
    tokio::pin!(fut);

    // 1) With no response yet, the None-await must remain pending: the
    //    test-side timer wins the race.
    let raced = tokio::time::timeout(Duration::from_millis(250), &mut fut).await;
    assert!(
        raced.is_err(),
        "await_pending_response(None) resolved on its own ({raced:?}) — None did NOT preserve unbounded wait"
    );

    // 2) Now deliver a response; the same (still-pending) future must resolve
    //    with the payload — the unbounded path still works end-to-end.
    let sender = state
        .pending
        .lock()
        .unwrap()
        .map
        .remove(&rid)
        .expect("sender present");
    sender
        .send(Ok(b"late-but-delivered".to_vec()))
        .expect("send");

    let out = tokio::time::timeout(Duration::from_secs(2), &mut fut)
        .await
        .expect("future should now resolve promptly")
        .expect("delivered payload");
    assert_eq!(out, b"late-but-delivered");
}

/// A `request_timeout = Some(large)` must NOT interfere with a request that is
/// answered before the deadline: the payload is returned normally.
#[tokio::test]
async fn request_timeout_generous_delivers_normally() {
    let state = make_state();
    let rid: u32 = 11;
    let (tx, rx) = oneshot::channel::<Result<Vec<u8>, ClientError>>();
    state.pending.lock().unwrap().map.insert(rid, tx);

    // Answer shortly after the await starts, well within the generous budget.
    let deliver = {
        let state = state.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let sender = state
                .pending
                .lock()
                .unwrap()
                .map
                .remove(&rid)
                .expect("sender present");
            sender.send(Ok(b"ok".to_vec())).expect("send");
        })
    };

    let out = await_pending_response(rx, Some(Duration::from_secs(5)), &state, rid)
        .await
        .expect("payload");
    assert_eq!(out, b"ok");

    deliver.await.expect("deliver task");
}

// ─── handshake_timeout (RV-CLIENT-NODE-1CFB3796) ─────────────────────────────

/// A full `Client::connect` against a peer that accepts TCP but never answers
/// the TLS ClientHello must fail with `HandshakeTimeout` once
/// `handshake_timeout` elapses — the pre-fix code bounded only the TCP
/// connect and left the TLS/SCRAM awaits unbounded.
#[tokio::test]
async fn rv_client_node_1cfb3796_handshake_deadline_bounds_silent_tls_peer() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral listener");
    let addr: SocketAddr = listener.local_addr().expect("listener addr");
    // Silent peer: accept, then stall forever without reading or writing.
    tokio::spawn(async move {
        let (_stream, _) = listener.accept().await.expect("accept");
        std::future::pending::<()>().await;
    });

    let budget = Duration::from_millis(250);
    let opts = ConnectOptions {
        addr,
        server_name: "localhost".into(),
        username: "timeout-probe".into(),
        password: Zeroizing::new(b"unused".to_vec()),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: None,
        handshake_timeout: Some(budget),
        request_timeout: None,
    };

    // Outer guard: on the pre-fix code the TLS await never resolves and this
    // test timer wins (failing the test); with the fix the handshake
    // deadline fires first.
    let raced = tokio::time::timeout(Duration::from_secs(5), Client::connect(opts)).await;
    let res = raced.expect("connect must respect handshake_timeout, not hang");
    match match res {
        Err(e) => e,
        Ok(_) => panic!("silent peer must fail the handshake"),
    } {
        ClientError::HandshakeTimeout(d, phase) => {
            assert_eq!(d, budget);
            assert_eq!(&*phase, "tls", "silent TLS peer must report the tls phase");
        }
        other => panic!("expected HandshakeTimeout, got {other:?}"),
    }
}

/// `bound_until` bounds a never-ready future — the deterministic stand-in
/// for any parked connect/write/response phase. Deadline elapses first:
/// `Ok(None)`, never a hang (RV-CLIENT-NODE-1CFB3796).
#[tokio::test(start_paused = true)]
async fn rv_client_node_1cfb3796_bound_until_bounds_never_ready_future() {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(250);
    let raced = tokio::time::timeout(
        Duration::from_secs(5),
        crate::client::bound_until(
            Some(deadline),
            std::future::pending::<Result<(), ClientError>>(),
        ),
    )
    .await;
    match raced.expect("bound_until must resolve, not hang") {
        Ok(None) => {}
        Ok(Some(Ok(()))) => panic!("a pending future cannot complete"),
        Ok(Some(Err(e))) => panic!("unexpected error from pending future: {e:?}"),
        Err(e) => panic!("unexpected error: {e:?}"),
    }
}

/// A request deadline elapsing while the WRITE is parked (1-byte duplex
/// buffer, peer never reads) yields `Ok(None)` and — because the dropped
/// future arms the mid-frame cancellation guard — CLOSES the connection
/// instead of leaving it reusable (RV-CLIENT-NODE-1CFB3796).
#[tokio::test(start_paused = true)]
async fn rv_client_node_1cfb3796_request_deadline_bounds_parked_write() {
    let (client_io, server_io) = tokio::io::duplex(1);
    let (_srv_r, _srv_w) = tokio::io::split(server_io); // peer never reads
    let (c_r, c_w) = tokio::io::split(client_io);
    let client = Client::for_test(c_r, c_w, LifecycleHooks::default());

    // The operation deadline (30 s) must elapse BEFORE the outer no-hang guard.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let raced = tokio::time::timeout(
        Duration::from_secs(120),
        crate::client::bound_until(Some(deadline), client.ping()),
    )
    .await;
    match raced.expect("bounded ping must resolve, not hang") {
        Ok(None) => {}
        Ok(Some(r)) => panic!("parked write must not succeed: {r:?}"),
        Err(e) => panic!("unexpected error from parked write: {e:?}"),
    }
    assert!(
        client.lifecycle_for_test().cause().is_some(),
        "timed-out write must close the connection"
    );
}

/// A request deadline elapsing while waiting on a HELD tokio mutex (the
/// Node client-mutex case) yields `Ok(None)` deterministically — no routed
/// addresses, no real server (RV-CLIENT-NODE-1CFB3796).
#[tokio::test(start_paused = true)]
async fn rv_client_node_1cfb3796_request_deadline_bounds_lock_wait() {
    let mutex = Arc::new(tokio::sync::Mutex::new(()));
    let guard = mutex.clone().lock_owned().await;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(250);
    let raced = tokio::time::timeout(
        Duration::from_secs(5),
        crate::client::bound_until(Some(deadline), async move {
            drop(mutex.lock().await);
            Ok::<(), ClientError>(())
        }),
    )
    .await;
    match raced.expect("bounded lock wait must resolve, not hang") {
        Ok(None) => {}
        Ok(Some(Ok(()))) => panic!("lock cannot be acquired while held"),
        Ok(Some(Err(e))) => panic!("unexpected error: {e:?}"),
        Err(e) => panic!("unexpected error: {e:?}"),
    }
    drop(guard);
}
