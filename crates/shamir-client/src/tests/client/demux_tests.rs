//! Unit tests for the rid-demux reader logic.
//!
//! We drive `reader_task` directly via `tokio::io::duplex`, injecting
//! hand-crafted response frames and asserting that each waiter receives
//! exactly the payload it expects, regardless of the order frames arrive.

use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;

use shamir_connect::common::envelope::{ErrorEnvelope, ResponseEnvelope};
use shamir_connect::common::push_envelope::{PushEnvelope, PushKind};
use shamir_transport_tcp::framing::write_frame;

use crate::client::subscription::EARLY_BUFFER_TOTAL_CAP;
use crate::client::transport::await_pending_response;
use crate::client::{reader_task, Lifecycle, LifecycleHooks, ShutdownGuard};
use crate::error::{ClientError, CloseCause};
use crate::subscription::SubscriptionHandle;

#[tokio::test]
async fn rv_client_b80c0d09_response_payload_wins_close_with_timeout() {
    let state = Lifecycle::new();
    let (tx, rx) = oneshot::channel();
    let _ = state.pending.map.insert_sync(79, tx);
    let response = state.pending.map.remove_sync(&79).expect("registered").1;
    response
        .send(Ok(b"payload".to_vec()))
        .expect("deliver payload");
    state.shutdown(CloseCause::PeerEof);
    let result =
        await_pending_response(rx, Some(std::time::Duration::from_secs(1)), &state, 79).await;
    assert_eq!(result.expect("payload wins close"), b"payload");
}

#[tokio::test]
async fn rv_client_b80c0d09_response_wins_close_when_both_ready_without_timeout() {
    let state = Lifecycle::new();
    let (tx, rx) = oneshot::channel();
    let _ = state.pending.map.insert_sync(77, tx);
    let response = state
        .pending
        .map
        .remove_sync(&77)
        .expect("registered response")
        .1;
    response
        .send(Ok(b"payload".to_vec()))
        .expect("deliver response");
    state.shutdown(CloseCause::PeerEof);
    let result = await_pending_response(rx, None, &state, 77).await;
    assert_eq!(result.expect("response wins"), b"payload");
}

#[tokio::test]
async fn rv_client_b80c0d09_response_wins_close_when_both_ready_with_timeout() {
    let state = Lifecycle::new();
    let (tx, rx) = oneshot::channel();
    let _ = state.pending.map.insert_sync(78, tx);
    let response = state
        .pending
        .map
        .remove_sync(&78)
        .expect("registered response")
        .1;
    response
        .send(Err(ClientError::Server {
            code: "expired".into(),
        }))
        .expect("deliver typed server error");
    state.shutdown(CloseCause::PeerEof);
    let result = crate::client::transport::await_pending_response(
        rx,
        Some(std::time::Duration::from_secs(1)),
        &state,
        78,
    )
    .await;
    assert!(matches!(result, Err(ClientError::Server { code }) if code == "expired"));
}
/// Write a [`ResponseEnvelope`] as a length-prefixed frame into `writer`.
async fn write_response(writer: &mut (impl AsyncWriteExt + Unpin), rid: u32, payload: &[u8]) {
    let env = ResponseEnvelope::ok(Some(rid), payload.to_vec());
    let bytes = env.to_msgpack().expect("encode response envelope");
    write_frame(writer, &bytes).await.expect("write_frame");
}

/// Write an [`ErrorEnvelope`] as a length-prefixed frame into `writer`.
async fn write_error(writer: &mut (impl AsyncWriteExt + Unpin), rid: Option<u32>, error: &str) {
    let env = ErrorEnvelope::new(rid, error);
    let bytes = env.to_msgpack().expect("encode error envelope");
    write_frame(writer, &bytes).await.expect("write_frame");
}

/// Write a garbage frame that cannot be decoded as either envelope type.
async fn write_garbage(writer: &mut (impl AsyncWriteExt + Unpin)) {
    let garbage = b"\xde\xad\xbe\xef not valid msgpack envelope";
    write_frame(writer, garbage).await.expect("write_frame");
}

/// Write a [`PushEnvelope`] as a length-prefixed frame into `writer`.
async fn write_push(writer: &mut (impl AsyncWriteExt + Unpin), envelope: &PushEnvelope) {
    let bytes = rmp_serde::to_vec_named(envelope).expect("encode push envelope");
    write_frame(writer, &bytes).await.expect("write_frame");
}

fn make_state() -> Lifecycle {
    Lifecycle::new()
}

fn register(state: &Lifecycle, rid: u32) -> oneshot::Receiver<Result<Vec<u8>, ClientError>> {
    let (tx, rx) = oneshot::channel();
    let _ = state.pending.map.insert_sync(rid, tx);
    rx
}

fn make_hooks() -> LifecycleHooks {
    LifecycleHooks::default()
}

/// The shutdown guard must be created BEFORE the task is spawned so a
/// cancellation before the reader's first poll still closes the lifecycle.
fn make_guard(state: &Lifecycle) -> ShutdownGuard {
    ShutdownGuard::new(state.clone())
}

// ─── Tests ───────────────────────────────────────────────────────────────────

/// Responses for rid=2 and rid=1 arrive in the wrong order; both callers
/// must receive their correct payloads.
#[tokio::test]
async fn demux_out_of_order_responses() {
    let state = make_state();
    let rx1 = register(&state, 1);
    let rx2 = register(&state, 2);

    let (mut writer, reader) = tokio::io::duplex(4096);

    // Spawn the reader task.
    let task = tokio::spawn(reader_task(
        reader,
        state.clone(),
        make_guard(&state),
        make_hooks(),
    ));

    // Send rid=2 first, then rid=1.
    write_response(&mut writer, 2, b"payload-2").await;
    write_response(&mut writer, 1, b"payload-1").await;

    // Close the writer end so reader_task sees EOF and exits.
    writer.shutdown().await.expect("shutdown writer");

    task.await.expect("reader_task panicked");

    let result1 = rx1.await.expect("rx1 dropped");
    let result2 = rx2.await.expect("rx2 dropped");

    assert_eq!(result1.unwrap(), b"payload-1");
    assert_eq!(result2.unwrap(), b"payload-2");
}

/// A frame without a rid (rid == None) must be silently dropped; the
/// two waiting callers must still receive their responses.
#[tokio::test]
async fn demux_frame_without_rid_is_dropped() {
    let state = make_state();
    let rx1 = register(&state, 1);
    let rx2 = register(&state, 2);

    let (mut writer, reader) = tokio::io::duplex(4096);

    let task = tokio::spawn(reader_task(
        reader,
        state.clone(),
        make_guard(&state),
        make_hooks(),
    ));

    // Inject a frame without a rid before the real responses.
    let no_rid_env = ResponseEnvelope::ok(None, b"push-notification".to_vec());
    let no_rid_bytes = no_rid_env.to_msgpack().unwrap();
    write_frame(&mut writer, &no_rid_bytes)
        .await
        .expect("write push frame");

    write_response(&mut writer, 1, b"p1").await;
    write_response(&mut writer, 2, b"p2").await;
    writer.shutdown().await.expect("shutdown");

    task.await.expect("reader_task panicked");

    assert_eq!(rx1.await.unwrap().unwrap(), b"p1");
    assert_eq!(rx2.await.unwrap().unwrap(), b"p2");
}

/// Invalid frames terminate the connection even while the peer remains open.
#[tokio::test]
async fn demux_garbage_frame_drains_waiters() {
    let state = make_state();
    let rx1 = register(&state, 1);
    let rx2 = register(&state, 2);
    let (mut writer, reader) = tokio::io::duplex(4096);
    let task = tokio::spawn(reader_task(
        reader,
        state.clone(),
        make_guard(&state),
        make_hooks(),
    ));
    write_garbage(&mut writer).await;
    let (a, b) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        (rx1.await.unwrap(), rx2.await.unwrap())
    })
    .await
    .expect("waiters must drain while peer stays open");
    assert!(matches!(
        a,
        Err(ClientError::ConnectionClosed(CloseCause::Protocol(_)))
    ));
    assert!(matches!(
        b,
        Err(ClientError::ConnectionClosed(CloseCause::Protocol(_)))
    ));
    task.await.expect("reader task");
    drop(writer);
}

#[tokio::test]
async fn ambiguous_duplicate_and_trailing_envelopes_close_connection() {
    let cases: &[&[u8]] = &[
        b"\x82\xa3res\xc4\x01x\xa5error\xa1e",
        b"\x82\xa3res\xc4\x01x\xa3res\xc4\x01y",
        b"\x81\xa3res\xc4\x01x\xc1",
    ];
    for frame in cases {
        let state = make_state();
        let rx = register(&state, 1);
        let (mut writer, reader) = tokio::io::duplex(4096);
        let task = tokio::spawn(reader_task(
            reader,
            state.clone(),
            make_guard(&state),
            make_hooks(),
        ));
        write_frame(&mut writer, frame).await.expect("write frame");
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(2), rx)
            .await
            .expect("terminal")
            .expect("sender");
        assert!(matches!(
            outcome,
            Err(ClientError::ConnectionClosed(CloseCause::Protocol(_)))
        ));
        task.await.expect("reader");
    }
}

/// On EOF, every in-flight waiter must receive
/// `ClientError::ConnectionClosed(CloseCause::PeerEof)`.
#[tokio::test]
async fn demux_eof_drains_all_pending() {
    let state = make_state();
    let rx1 = register(&state, 1);
    let rx2 = register(&state, 2);

    let (writer, reader) = tokio::io::duplex(4096);

    // Drop the writer immediately — reader sees EOF right away.
    drop(writer);

    let task = tokio::spawn(reader_task(
        reader,
        state.clone(),
        make_guard(&state),
        make_hooks(),
    ));
    task.await.expect("reader_task panicked");

    // Both waiters should have gotten ConnectionClosed(PeerEof).
    let err1 = rx1.await.unwrap().unwrap_err();
    let err2 = rx2.await.unwrap().unwrap_err();
    assert!(
        matches!(err1, ClientError::ConnectionClosed(CloseCause::PeerEof)),
        "expected ConnectionClosed(PeerEof), got {err1:?}"
    );
    assert!(
        matches!(err2, ClientError::ConnectionClosed(CloseCause::PeerEof)),
        "expected ConnectionClosed(PeerEof), got {err2:?}"
    );

    // Fast-path hint must be set.
    assert!(state.closed_hint.load(std::sync::atomic::Ordering::Acquire));
}

/// An error envelope (server-level error) is routed to the matching waiter
/// as the typed `ClientError::Server { code }` (RV-CLIENT-AE50D83D: the
/// envelope's single string IS the spec §14 code).
#[tokio::test]
async fn demux_error_envelope_routed_to_waiter() {
    let state = make_state();
    let rx = register(&state, 7);

    let (mut writer, reader) = tokio::io::duplex(4096);

    let task = tokio::spawn(reader_task(
        reader,
        state.clone(),
        make_guard(&state),
        make_hooks(),
    ));

    write_error(&mut writer, Some(7), "session_expired").await;
    writer.shutdown().await.expect("shutdown");

    task.await.expect("reader_task panicked");

    let err = rx.await.unwrap().unwrap_err();
    assert!(
        matches!(err, ClientError::Server { ref code } if code == "session_expired"),
        "unexpected error: {err:?}"
    );
}

/// RV-CLIENT-AE50D83D: an envelope-level server error surfaces as the typed
/// `ClientError::Server` carrying the spec §14 code — NOT the old
/// stringly-typed `ClientError::Protocol(format!("server error envelope: …"))`.
#[tokio::test]
async fn rv_client_ae50d83d_server_error_envelope_typed() {
    let state = make_state();
    let rx = register(&state, 9);

    let (mut writer, reader) = tokio::io::duplex(4096);
    let task = tokio::spawn(reader_task(
        reader,
        state.clone(),
        make_guard(&state),
        make_hooks(),
    ));

    write_error(&mut writer, Some(9), "session_expired").await;
    writer.shutdown().await.expect("shutdown");
    task.await.expect("reader_task panicked");

    let err = rx.await.unwrap().unwrap_err();
    match err {
        ClientError::Server { code } => assert_eq!(code, "session_expired"),
        other => {
            panic!("expected ClientError::Server {{ code: \"session_expired\" }}, got {other:?}")
        }
    }
}

/// RV-CLIENT-D5B9F8A3: pushes for MANY DISTINCT unknown sub ids must be
/// bounded by the AGGREGATE early-buffer cap, not just the per-key cap —
/// one push per key keeps every per-key buffer tiny while the number of
/// keys (server-controlled) grows without bound. Post-fix the total number
/// of buffered envelopes never exceeds `EARLY_BUFFER_TOTAL_CAP`.
#[tokio::test]
async fn rv_client_d5b9f8a3_early_buffer_aggregate_bound() {
    let state = make_state();
    let rx = register(&state, 1);

    // Large buffer: all frames must be writable without backpressure
    // deadlocking the sequential writer loop.
    let (mut writer, reader) = tokio::io::duplex(256 * 1024);
    let task = tokio::spawn(reader_task(
        reader,
        state.clone(),
        make_guard(&state),
        make_hooks(),
    ));

    // More DISTINCT keys than the aggregate bound — one push each, so the
    // per-key cap (256) is never the binding constraint.
    let distinct = EARLY_BUFFER_TOTAL_CAP + 256;
    for sub in 0..distinct as u64 {
        let push = PushEnvelope {
            push: PushKind::Event,
            sub,
            seq: 0,
            data: None,
            gap_at: None,
        };
        write_push(&mut writer, &push).await;
    }

    // In-order sync point: the rid=1 response proves every push frame was
    // already routed (the reader processes frames in order).
    write_response(&mut writer, 1, b"ok").await;
    assert_eq!(rx.await.unwrap().unwrap(), b"ok");

    {
        let total = state.subs.early_total();
        assert_eq!(state.subs.debug_early_total(), total);
        assert!(total <= EARLY_BUFFER_TOTAL_CAP);
        let denied = EARLY_BUFFER_TOTAL_CAP as u64;
        state.subs.route(PushEnvelope {
            push: PushKind::Event,
            sub: denied,
            seq: 1,
            data: None,
            gap_at: None,
        });
        assert!(
            !state.subs.contains(&denied),
            "denied key must not be retained"
        );
        state.shutdown(CloseCause::PeerEof);
        assert_eq!(state.subs.early_total(), 0);
        assert_eq!(state.subs.debug_early_total(), 0);
    }

    writer.shutdown().await.expect("shutdown");
    task.await.expect("reader_task panicked");
}

/// A late response for a rid that no longer has a waiter (e.g., timed-out
/// or cancelled request) must be silently dropped — no panic, no hang.
#[tokio::test]
async fn demux_late_response_for_unknown_rid_is_dropped() {
    // Only rid=1 is registered; server sends rid=99 (orphan) + rid=1.
    let state = make_state();
    let rx1 = register(&state, 1);

    let (mut writer, reader) = tokio::io::duplex(4096);

    let task = tokio::spawn(reader_task(
        reader,
        state.clone(),
        make_guard(&state),
        make_hooks(),
    ));

    // orphan frame
    write_response(&mut writer, 99, b"orphan").await;
    // valid frame
    write_response(&mut writer, 1, b"mine").await;
    writer.shutdown().await.expect("shutdown");

    task.await.expect("reader_task panicked");

    // rid=1 still got its payload correctly.
    assert_eq!(rx1.await.unwrap().unwrap(), b"mine");
}

// ─── Push-frame demux tests ────────────────────────────────────────────────

/// A push frame routes to a registered subscription handle.
#[tokio::test]
async fn push_frame_routes_to_registered_subscription() {
    let state = make_state();

    // Register sub_id=42
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let (term_tx, _term_rx) = oneshot::channel();
    state
        .subs
        .register(42, tx.clone(), term_tx)
        .expect("register route");

    let (mut writer, reader) = tokio::io::duplex(4096);
    let task = tokio::spawn(reader_task(
        reader,
        state.clone(),
        make_guard(&state),
        make_hooks(),
    ));

    let push = PushEnvelope {
        push: PushKind::Event,
        sub: 42,
        seq: 1,
        data: Some(b"hello".to_vec()),
        gap_at: None,
    };
    write_push(&mut writer, &push).await;
    writer.shutdown().await.expect("shutdown");

    task.await.expect("reader_task panicked");

    let received = rx.recv().await.expect("should receive push");
    assert_eq!(received.sub, 42);
    assert_eq!(received.seq, 1);
    assert_eq!(received.data, Some(b"hello".to_vec()));
}

/// A push frame for an unregistered sub_id is buffered — no panic, no loss.
#[tokio::test]
async fn push_frame_for_unknown_sub_is_buffered() {
    let state = make_state();
    let rx1 = register(&state, 1);

    let (mut writer, reader) = tokio::io::duplex(4096);
    let task = tokio::spawn(reader_task(
        reader,
        state.clone(),
        make_guard(&state),
        make_hooks(),
    ));

    let push = PushEnvelope {
        push: PushKind::Ready,
        sub: 999,
        seq: 0,
        data: None,
        gap_at: None,
    };
    write_push(&mut writer, &push).await;

    // The awaited rid=1 response proves in-order processing of the earlier
    // push frame. The reader is STILL running when this resolves.
    write_response(&mut writer, 1, b"ok").await;
    assert_eq!(rx1.await.unwrap().unwrap(), b"ok");

    // Early buffer holds the push WHILE the reader is running.
    assert_eq!(state.subs.early_total(), 1);

    // Terminate the reader: its shutdown transition clears the early buffer.
    writer.shutdown().await.expect("shutdown");
    task.await.expect("reader_task panicked");
    assert!(state.subs.is_empty());
    assert_eq!(state.subs.early_total(), 0);
}

/// A stalled consumer (never calling `recv`) must not balloon memory:
/// the bounded mpsc channel drops new pushes (with a warn log) past its
/// capacity. We use a small explicit cap so the test is fast and
/// deterministic, independent of `CLIENT_SUB_CHANNEL_CAP` tuning.
#[tokio::test]
async fn push_frame_bounded_channel_drops_on_full() {
    let state = make_state();

    // Tiny cap to exercise the full-channel branch quickly.
    let cap: usize = 4;
    let (tx, mut rx) = tokio::sync::mpsc::channel(cap);
    let (term_tx, _term_rx) = oneshot::channel();
    state.subs.register(11, tx, term_tx).expect("live route");

    let (mut writer, reader) = tokio::io::duplex(64 * 1024);
    let task = tokio::spawn(reader_task(
        reader,
        state.clone(),
        make_guard(&state),
        make_hooks(),
    ));

    // Stream many push frames without the consumer calling recv. Tokio's
    // bounded mpsc may admit a small implementation-defined slack beyond
    // `cap`, so we assert "no more than cap + small slack" rather than
    // exact equality, and verify no panic occurs.
    let n_writes: usize = cap * 8;
    for seq in 0..n_writes {
        let env = PushEnvelope {
            push: PushKind::Event,
            sub: 11,
            seq: seq as u64,
            data: None,
            gap_at: None,
        };
        write_push(&mut writer, &env).await;
    }
    writer.shutdown().await.expect("shutdown");
    task.await.expect("reader_task panicked");

    // Drain whatever the channel admitted.
    let mut delivered: usize = 0;
    while rx.try_recv().is_ok() {
        delivered += 1;
    }

    assert!(
        delivered >= cap && delivered <= cap + 2,
        "bounded channel should admit ~cap entries, got {delivered} (cap={cap})"
    );
}

/// Stale handle drop must leave a later registration routed and terminable.
#[test]
fn rv_client_b80c0d09_stale_handle_drop_preserves_reregistered_route() {
    let lifecycle = Lifecycle::new();
    let (old_tx, old_rx) = tokio::sync::mpsc::channel(8);
    let (old_term_tx, old_term_rx) = oneshot::channel();
    lifecycle
        .subs
        .register(19, old_tx.clone(), old_term_tx)
        .expect("old route");
    let old = SubscriptionHandle::new(
        19,
        old_rx,
        old_tx.downgrade(),
        lifecycle.subs.clone(),
        old_term_rx,
    );
    let (new_tx, mut new_rx) = tokio::sync::mpsc::channel(8);
    let (new_term_tx, mut new_term_rx) = oneshot::channel();
    lifecycle
        .subs
        .register(19, new_tx.clone(), new_term_tx)
        .expect("new route");
    drop(old);
    lifecycle.subs.route(PushEnvelope {
        push: PushKind::Event,
        sub: 19,
        seq: 1,
        data: None,
        gap_at: None,
    });
    assert_eq!(new_rx.try_recv().expect("new channel retained").seq, 1);
    lifecycle.shutdown(CloseCause::PeerEof);
    assert_eq!(new_term_rx.try_recv(), Ok(CloseCause::PeerEof));
}

/// Dropping a handle removes only its own route.
#[tokio::test]
async fn subscription_handle_drop_removes_from_registry() {
    let lifecycle = Lifecycle::new();
    let (tx, rx) = tokio::sync::mpsc::channel::<PushEnvelope>(16);
    let (term_tx, term_rx) = oneshot::channel::<CloseCause>();
    lifecycle
        .subs
        .register(7, tx.clone(), term_tx)
        .expect("route");
    let handle = SubscriptionHandle::new(7, rx, tx.downgrade(), lifecycle.subs.clone(), term_rx);
    assert!(lifecycle.subs.contains(&7));
    drop(handle);
    assert!(!lifecycle.subs.contains(&7));
}
