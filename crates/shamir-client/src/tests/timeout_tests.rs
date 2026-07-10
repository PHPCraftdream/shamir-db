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

use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use shamir_collections::TFxMap;
use tokio::sync::oneshot;

use crate::client::{await_pending_response, connect_tcp, PendingMap};
use crate::error::ClientError;

/// A guaranteed-unroutable private black-hole address: TCP `connect` to it
/// neither completes nor is refused quickly (the SYN is dropped), so a
/// bounded `connect_timeout` is the only thing that unblocks the caller.
const BLACKHOLE: &str = "10.255.255.1:9";

fn make_pending() -> PendingMap {
    Arc::new(StdMutex::new(TFxMap::default()))
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
    let pending = make_pending();
    let rid: u32 = 7;
    // Register a waiter but NEVER send on `tx` (simulate a server that
    // accepts the connection but never answers this rid). We keep `tx` alive
    // in the map, otherwise the receiver would resolve with
    // `ConnectionClosed` instead of timing out.
    let (tx, rx) = oneshot::channel::<Result<Vec<u8>, ClientError>>();
    pending.lock().unwrap().insert(rid, tx);

    let budget = Duration::from_millis(200);
    let start = Instant::now();
    let res = await_pending_response(rx, Some(budget), &pending, rid).await;
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
        !pending.lock().unwrap().contains_key(&rid),
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
    let pending = make_pending();
    let rid: u32 = 3;
    let (tx, rx) = oneshot::channel::<Result<Vec<u8>, ClientError>>();
    pending.lock().unwrap().insert(rid, tx);

    let fut = await_pending_response(rx, None, &pending, rid);
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
    let sender = pending.lock().unwrap().remove(&rid).expect("sender present");
    sender.send(Ok(b"late-but-delivered".to_vec())).expect("send");

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
    let pending = make_pending();
    let rid: u32 = 11;
    let (tx, rx) = oneshot::channel::<Result<Vec<u8>, ClientError>>();
    pending.lock().unwrap().insert(rid, tx);

    // Answer shortly after the await starts, well within the generous budget.
    let deliver = {
        let pending = pending.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let sender = pending.lock().unwrap().remove(&rid).expect("sender present");
            sender.send(Ok(b"ok".to_vec())).expect("send");
        })
    };

    let out = await_pending_response(rx, Some(Duration::from_secs(5)), &pending, rid)
        .await
        .expect("payload");
    assert_eq!(out, b"ok");

    deliver.await.expect("deliver task");
}
