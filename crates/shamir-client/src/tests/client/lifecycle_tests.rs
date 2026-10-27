//! P0 C1 lifecycle regression tests (cards RV-CLIENT-93C26EBF, -B42B8576,
//! -E38BE183) driven through the REAL production paths: `Client::for_test`
//! → `assemble` → `reader_task`/`roundtrip`/`subscribe_push`, over injected
//! duplex pipes with SEPARATE server→client and client→server directions, so
//! a read EOF never masks whether a write happened. Deterministic via
//! `LifecycleHooks` (one-shot pairs and multi-fire points) — no sleeps, and
//! every assertion targets the receive side (no registry inspection).

use std::pin::{pin, Pin};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{duplex, split, AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use tokio::sync::oneshot;

use shamir_connect::common::envelope::ResponseEnvelope;
use shamir_connect::common::push_envelope::{PushEnvelope, PushKind};
use shamir_query_types::wire::DbRequest;
use shamir_transport_tcp::framing::{read_frame, write_frame, MAX_FRAME_SIZE_DEFAULT};

use crate::client::{reader_task, HookPoint, Lifecycle, LifecycleHooks, ShutdownGuard};
use crate::error::{ClientError, CloseCause};
use crate::subscription::SubscriptionHandle;
use crate::{Client, DbResponse};

const CLIENT_BUF: usize = 64 * 1024;
const SERVER_OUT_BUF: usize = 512 * 1024;

fn push(sub: u64, seq: u64) -> PushEnvelope {
    PushEnvelope {
        push: PushKind::Event,
        sub,
        seq,
        data: Some(b"evt".to_vec()),
        gap_at: None,
    }
}

fn pong_payload() -> Vec<u8> {
    rmp_serde::to_vec_named(&DbResponse::Pong).expect("encode pong")
}

async fn write_push(server_out: &mut (impl AsyncWrite + Unpin), env: &PushEnvelope) {
    let bytes = rmp_serde::to_vec_named(env).expect("encode push");
    write_frame(server_out, &bytes).await.expect("write push");
}

async fn write_response(server_out: &mut (impl AsyncWrite + Unpin), rid: u32, payload: &[u8]) {
    let bytes = ResponseEnvelope::ok(Some(rid), payload.to_vec())
        .to_msgpack()
        .expect("encode envelope");
    write_frame(server_out, &bytes).await.expect("write frame");
}

/// The server-facing pipe ends: `server_out` writes frames INTO the client
/// (dropping it ⇒ read EOF for the client's reader); `server_in` observes
/// what the client writes and stays open across the reader's EOF.
struct ServerEnds<S, R> {
    server_out: S,
    server_in: R,
}

fn setup(
    hooks: LifecycleHooks,
) -> (
    Client,
    ServerEnds<impl AsyncWrite + Unpin, impl AsyncRead + Unpin>,
) {
    // client→server direction: the client writes; the test reads.
    let (c2s_client_side, c2s_server_side) = duplex(CLIENT_BUF);
    // server→client direction: the test writes; the client reads.
    let (s2c_client_side, s2c_server_side) = duplex(SERVER_OUT_BUF);

    let (client_reader, _unused_s2c_write) = split(s2c_client_side);
    let (_unused_c2s_read, client_writer) = split(c2s_client_side);

    let client = Client::for_test(client_reader, client_writer, hooks);
    (
        client,
        ServerEnds {
            server_out: s2c_server_side,
            server_in: c2s_server_side,
        },
    )
}

fn assert_closed(err: &ClientError, cause: &CloseCause) {
    assert!(
        matches!(err, ClientError::ConnectionClosed(c)
            if std::mem::discriminant(c) == std::mem::discriminant(cause)),
        "expected ConnectionClosed({cause}), got {err:?}"
    );
}

/// Categorical no-write proof: drop the client (closing the pipe's write
/// end) and read ONE byte from the client→server direction — the read must
/// end the pipe with 0 bytes, i.e. NOTHING was ever written.
async fn assert_no_bytes_written<R>(server_in: &mut R)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buf = [0u8; 1];
    let n = server_in.read(&mut buf).await.expect("read pipe");
    assert_eq!(
        n, 0,
        "zero bytes may be written after close, got {n} byte(s)"
    );
}

// ─── T1 — RV-CLIENT-93C26EBF ────────────────────────────────────────────────

/// T1: 8 live subscriptions and 8 in-flight requests, then EOF. Every ping
/// is registered AND polled before the EOF (multi-fire after_register hook);
/// each resolves `ConnectionClosed(PeerEof)`; every handle yields its queued
/// push then `None`, with `termination() == Some(PeerEof)`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rv_client_93c26ebf_eof_closes_subs_and_requests() {
    let (reg_hook, mut reg_reached) = HookPoint::new_multi(8);
    let hooks = LifecycleHooks {
        after_register: Some(reg_hook),
        ..Default::default()
    };
    let (mut client, mut ends) = setup(hooks);
    let reader = client.take_reader_handle().expect("reader handle present");
    let mut handles: Vec<SubscriptionHandle> = Vec::new();
    for sub in 1..=8u64 {
        handles.push(client.subscribe_push(sub));
    }
    // Spawn the pings as tasks so all 8 are registered AND polled before the
    // EOF (Arc-shared client: spawn needs 'static).
    let client = Arc::new(client);
    let pings: Vec<_> = (0..8)
        .map(|_| {
            let c = Arc::clone(&client);
            tokio::spawn(async move { c.ping().await })
        })
        .collect();

    // Queue one push per subscription, wait until EVERY ping is registered
    // (and past registration), then disconnect.
    for sub in 1..=8u64 {
        write_push(&mut ends.server_out, &push(sub, 1)).await;
    }
    for _ in 0..8 {
        reg_reached
            .recv()
            .await
            .expect("every ping must reach after_register");
    }
    let ServerEnds { server_out, .. } = ends;
    drop(server_out);

    reader.await.expect("reader_task panicked");

    for handle in pings {
        let res = handle.await.expect("ping task panicked");
        let err = res.expect_err("ping must fail after EOF");
        assert_closed(&err, &CloseCause::PeerEof);
    }
    for mut handle in handles {
        let item = handle
            .next()
            .await
            .expect("queued push must be delivered before the stream ends");
        assert_eq!(item.data.as_deref(), Some(&b"evt"[..]));
        assert!(handle.next().await.is_none(), "stream must end");
        assert_eq!(handle.termination(), Some(CloseCause::PeerEof));
    }
}

// ─── T2 — RV-CLIENT-B42B8576 ────────────────────────────────────────────────

/// T2a: REGISTRATION AFTER the old drain completed — the ping parks at
/// `AfterFastCheck`, the EOF-driven shutdown fully completes (reader task
/// joined), and only then is the ping released: it must observe the closed
/// registry and resolve `PeerEof` writing NOTHING to the wire (categorical
/// zero-byte proof).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rv_client_b42b8576_register_after_drain() {
    let (hook, mut reached, release) = HookPoint::new();
    let hooks = LifecycleHooks {
        after_fast_check: Some(hook),
        ..Default::default()
    };
    let (mut client, mut ends) = setup(hooks);
    let reader = client.take_reader_handle().expect("reader handle");

    // Advance the ping exactly to the post-fast-check pause point.
    {
        let mut ping = pin!(client.ping());
        tokio::select! {
            r = &mut ping => panic!("ping resolved before release: {r:?}"),
            _ = &mut reached => {}
        }

        // EOF while the ping is parked, and the reader FULLY completes (the
        // old-code drain already ran BEFORE the registration is released).
        drop(ends.server_out);
        reader.await.expect("reader_task panicked");

        // Now let the ping proceed: it must observe the closed registry
        // before writing anything.
        release.send(()).expect("release after_fast_check");
        tokio::select! {
            biased;
            r = &mut ping => {
                let err = r.expect_err("ping must fail after close");
                assert_closed(&err, &CloseCause::PeerEof);
            }
            frame = read_frame(&mut ends.server_in, MAX_FRAME_SIZE_DEFAULT) => {
                panic!("request bytes were written after close: {frame:?}");
            }
        }
    }

    // Categorical: drop the client and prove ZERO bytes ever reached the
    // client→server direction.
    drop(client);
    assert_no_bytes_written(&mut ends.server_in).await;
}

/// T2b: same parking seam, but the no-write assertion is categorical: after
/// the ping resolves `PeerEof`, dropping the client must leave the
/// client→server pipe at end-of-stream with 0 bytes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rv_client_b42b8576_no_write_after_close() {
    let (hook, mut reached, release) = HookPoint::new();
    let hooks = LifecycleHooks {
        after_fast_check: Some(hook),
        ..Default::default()
    };
    let (mut client, mut ends) = setup(hooks);
    let reader = client.take_reader_handle().expect("reader handle");

    {
        let mut ping = pin!(client.ping());
        tokio::select! {
            r = &mut ping => panic!("ping resolved before release: {r:?}"),
            _ = &mut reached => {}
        }

        drop(ends.server_out);
        reader.await.expect("reader_task panicked");

        release.send(()).expect("release after_fast_check");
        let err = ping.await.expect_err("ping must fail after close");
        assert_closed(&err, &CloseCause::PeerEof);
    }

    drop(client);
    assert_no_bytes_written(&mut ends.server_in).await;
}

/// Registration WINS the boundary: the reader is parked at
/// `ReaderBeforeShutdown` (pre-close-transition, outside the registry lock)
/// while a REAL ping registers through the production path; releasing the
/// reader runs the shutdown and the just-registered ping resolves `PeerEof`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rv_client_b42b8576_registration_wins_boundary() {
    let (reader_hook, mut reader_reached, reader_release) = HookPoint::new();
    let (reg_hook, mut reg_reached, reg_release) = HookPoint::new();
    let hooks = LifecycleHooks {
        reader_before_shutdown: Some(reader_hook),
        after_register: Some(reg_hook),
        ..Default::default()
    };
    let (client, ends) = setup(hooks);

    // Reader hits EOF and parks BEFORE the shutdown transition.
    drop(ends.server_out);
    (&mut reader_reached)
        .await
        .expect("reader parked pre-close");

    // A REAL request registers through the production path while the reader
    // is parked pre-close.
    let mut ping = pin!(client.ping());
    tokio::select! {
        r = &mut ping => panic!("ping resolved before the drain: {r:?}"),
        _ = &mut reg_reached => {}
    }

    // Release the reader: shutdown runs NOW and must own the just-inserted
    // entry; the released ping joins the cleanup with the same cause.
    reader_release.send(()).expect("release reader hook");
    reg_release.send(()).expect("release register hook");
    let err = ping.await.expect_err("drained ping must fail");
    assert_closed(&err, &CloseCause::PeerEof);
}

/// THE write-phase bug: a ping blocked on WRITER ACQUISITION when shutdown
/// fires must resolve `ConnectionClosed(PeerEof)` WITHOUT the writer being
/// released — the writer-acquisition race against the closure watch. Fails
/// on old code: the ping stays parked on the mutex forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rv_client_b42b8576_write_phase_shutdown_unblocks() {
    let (reg_hook, mut reg_reached) = HookPoint::new_multi(1);
    let hooks = LifecycleHooks {
        after_register: Some(reg_hook),
        ..Default::default()
    };
    let (client, ends) = setup(hooks);
    // Hold the write mutex so the ping parks on writer acquisition (Arc for
    // the 'static spawn).
    let client = Arc::new(client);
    let writer_guard = client.hold_writer_for_test().await;

    let pinger = Arc::clone(&client);
    let ping = tokio::spawn(async move { pinger.ping().await });
    reg_reached
        .recv()
        .await
        .expect("ping registered; it is now parked on the writer");

    drop(ends.server_out); // EOF → shutdown

    // The ping must beat the timeout — the closure, not the writer release,
    // unblocks it.
    match tokio::time::timeout(Duration::from_secs(5), ping).await {
        Ok(joined) => {
            let res = joined.expect("ping task panicked");
            let err = res.expect_err("ping must fail");
            assert_closed(&err, &CloseCause::PeerEof);
        }
        Err(_elapsed) => panic!("ping stayed blocked on the writer past shutdown"),
    }
    drop(writer_guard);
}

// ─── T3/T4 — RV-CLIENT-E38BE183 ─────────────────────────────────────────────

/// T3: calls on an already-closed client fail with the stored cause and
/// write zero bytes; `subscribe_push` on a closed client yields an
/// already-ended handle.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rv_client_e38be183_closed_client_calls() {
    let (mut client, mut ends) = setup(LifecycleHooks::default());
    let reader = client.take_reader_handle().expect("reader handle");
    drop(ends.server_out); // EOF
    reader.await.expect("reader_task panicked");

    let err = client.ping().await.expect_err("closed client must fail");
    assert_closed(&err, &CloseCause::PeerEof);

    let mut handle = client.subscribe_push(7);
    assert!(handle.next().await.is_none(), "closed handle ends at once");
    assert_eq!(handle.termination(), Some(CloseCause::PeerEof));

    // Prove nothing was ever written.
    drop(client);
    assert_no_bytes_written(&mut ends.server_in).await;
}

/// T4: a subscription handle outlives its client. The ping is spawned and
/// registered FIRST (multi-fire after_register signal), then the push and
/// the pong are written — in-order processing delivers the queued push to
/// the handle. Dropping the client terminates the handle with
/// `ClientDropped`; an explicit `close()` with `LocalClose`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rv_client_e38be183_handle_outlives_client() {
    for (label, close_explicitly) in [("drop", false), ("close", true)] {
        let (reg_hook, mut reg_reached) = HookPoint::new_multi(1);
        let hooks = LifecycleHooks {
            after_register: Some(reg_hook),
            ..Default::default()
        };
        let (client, mut ends) = setup(hooks);
        let client = Arc::new(client);
        let mut handle = client.subscribe_push(5);
        // Register the ping FIRST, then write the push + pong.
        let pinger = Arc::clone(&client);
        let ping = tokio::spawn(async move { pinger.ping().await });
        reg_reached.recv().await.expect("ping registered");
        write_push(&mut ends.server_out, &push(5, 1)).await;
        write_response(&mut ends.server_out, 1, &pong_payload()).await;
        ping.await.expect("ping task").expect("pong");

        if close_explicitly {
            // Sole Arc owner (the ping task was joined): take the client back
            // for the explicit close.
            let owned = Arc::try_unwrap(client).ok().expect("sole client owner");
            owned.close().await;
        } else {
            drop(client);
        }

        let expected = if close_explicitly {
            CloseCause::LocalClose
        } else {
            CloseCause::ClientDropped
        };
        let item = handle.next().await.expect("queued push delivered");
        assert_eq!(item.sub, 5, "{label}");
        assert!(handle.next().await.is_none(), "{label}: stream must end");
        assert_eq!(
            handle.termination(),
            Some(expected),
            "{label}: wrong termination cause"
        );
    }
}

/// T5a: CAP+1 pushes with no consumer, then EOF: exactly CAP items are
/// delivered in order, then the stream ends with `PeerEof`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rv_client_e38be183_push_full_channel_then_eof() {
    let cap = crate::client::subscription::CLIENT_SUB_CHANNEL_CAP;
    let (mut client, mut ends) = setup(LifecycleHooks::default());
    let mut handle = client.subscribe_push(9);

    for seq in 0..(cap as u64 + 1) {
        write_push(&mut ends.server_out, &push(9, seq)).await;
    }
    drop(ends.server_out);
    let reader = client.take_reader_handle().expect("reader handle");
    reader.await.expect("reader_task panicked");

    let mut delivered = 0;
    while let Some(env) = handle.next().await {
        assert_eq!(env.seq, delivered as u64, "pushes stay ordered");
        delivered += 1;
    }
    assert_eq!(delivered, cap, "exactly CAP queued pushes survive");
    assert_eq!(handle.termination(), Some(CloseCause::PeerEof));
}

/// T5b: pushes early-buffered for an unregistered sub, then EOF, THEN
/// `subscribe_push`: the closed client returns an already-ended handle (the
/// early buffer died with the connection).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rv_client_e38be183_early_buffer_then_eof_subscribe() {
    let (mut client, mut ends) = setup(LifecycleHooks::default());
    write_push(&mut ends.server_out, &push(9, 0)).await;
    write_push(&mut ends.server_out, &push(9, 1)).await;
    drop(ends.server_out);
    let reader = client.take_reader_handle().expect("reader handle");
    reader.await.expect("reader_task panicked");

    let mut handle = client.subscribe_push(9);
    assert!(
        handle.next().await.is_none(),
        "no early-buffered push may be delivered after shutdown"
    );
    assert_eq!(handle.termination(), Some(CloseCause::PeerEof));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rv_client_2d08f353_successful_subscription_flushes_early_push_variant() {
    let (client, mut ends) = setup(LifecycleHooks::default());
    write_push(&mut ends.server_out, &push(71, 1)).await;
    // The reader handles frames in order: once the first ping (rid 1) resolves,
    // the push above is already in the early buffer.
    let pong = pong_payload();
    let (ping, _) = tokio::join!(
        client.ping(),
        write_response(&mut ends.server_out, 1, &pong)
    );
    ping.expect("ping");
    let mut subscription = client.subscribe_push(71);
    let received = subscription.next().await.expect("buffered push flushed");
    assert_eq!(received.sub, 71);
    assert_eq!(received.seq, 1);
    client.close().await;
}

/// A reader that parks forever on its first poll — keeps `reader_task`
/// suspended at a known point for the abort test.
struct FirstPollSignal {
    fired: Option<oneshot::Sender<()>>,
}

impl AsyncRead for FirstPollSignal {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if let Some(tx) = self.get_mut().fired.take() {
            let _ = tx.send(());
        }
        Poll::Pending
    }
}

/// T5c: aborting the PRODUCTION reader task (spawned by `Client::for_test` →
/// `assemble`) cancels the lifecycle with `ReaderCancelled`: a subsequent
/// ping and a `subscribe_push` handle both carry exactly that cause.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rv_client_e38be183_reader_abort_cancels_lifecycle() {
    let (mut client, ends) = setup(LifecycleHooks::default());
    let reader = client.take_reader_handle().expect("reader handle");
    // The pipe stays FULLY OPEN until after the abort: the reader is parked
    // on its first read → deterministically cancellable. Dropping `ends`
    // first would race EOF against the cancellation on a multithreaded
    // runtime (the reader could win with PeerEof).
    reader.abort();
    let joined = reader.await;
    assert!(joined.is_err(), "aborted reader task must be cancelled");

    // The dropped ShutdownGuard ran shutdown(ReaderCancelled).
    let err = client.ping().await.expect_err("closed client must fail");
    assert_closed(&err, &CloseCause::ReaderCancelled);

    let mut handle = client.subscribe_push(3);
    assert!(handle.next().await.is_none(), "handle ends at once");
    assert_eq!(handle.termination(), Some(CloseCause::ReaderCancelled));

    // Only NOW tear the pipe down — after every assertion above.
    drop(ends);
}

/// (P0 D) A `reader_task` future dropped BEFORE its first poll still closes
/// the lifecycle: the `ShutdownGuard` is constructed by the caller
/// (`assemble`, or this test) BEFORE the future can be dropped unpolled.
/// This is the ONLY test allowed to inspect lifecycle state directly — it
/// unit-tests the guard seam itself, not a client oracle.
#[tokio::test]
async fn client_reader_dropped_before_poll_closes_lifecycle() {
    let state = Lifecycle::new();
    let guard = ShutdownGuard::new(state.clone());
    let reader = FirstPollSignal { fired: None };
    let fut = reader_task(reader, state.clone(), guard, LifecycleHooks::default());
    drop(fut); // dropped WITHOUT ever being polled

    assert_eq!(state.cause(), Some(CloseCause::ReaderCancelled));
    assert!(state.closed_hint.load(std::sync::atomic::Ordering::Acquire));
}

// ─── Lost-wakeup regression ───────────────────────────────────────────────

/// `shutdown` publishes via `send_replace` (watch `send` FAILS and drops the
/// value when no receiver exists yet), and `closed_wait` checks the stored
/// value right after subscribing. Here shutdown completes with NO
/// `closed_wait` subscriber in existence; a later waiter must still resolve
/// the stored cause.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_shutdown_before_waiter_still_observes_cause() {
    let (mut client, ends) = setup(LifecycleHooks::default());
    let reader = client.take_reader_handle().expect("reader handle");
    drop(ends.server_out); // EOF → shutdown (no closed_wait receiver yet)
    reader.await.expect("reader_task panicked");

    let cause = tokio::time::timeout(
        Duration::from_secs(5),
        client.lifecycle_for_test().closed_wait(),
    )
    .await
    .expect("closed_wait must resolve a cause stored before subscribing");
    assert!(matches!(cause, CloseCause::PeerEof), "got {cause:?}");
}

// ─── Mid-frame caller cancellation ─────────────────────────────────────

/// A caller future dropped MID-FRAME must leave the connection closed, not
/// reusable: the `WriteCancelGuard`'s Drop shuts the lifecycle down with an
/// `Io` cause. The emulated peer reads exactly ONE byte (≥1 byte was
/// transmitted → the write is mid-frame and blocked on the tiny 1024-byte
/// c2s buffer), then the task is aborted. The s2c direction stays open so
/// the reader never sees EOF — the ONLY close is the cancellation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_cancelled_midframe_write_closes_lifecycle() {
    // Mirror `setup` but with a TINY client→server buffer; keep the s2c
    // write half alive (dropping it would EOF the reader).
    let (c2s_client, mut c2s_server) = duplex(1024);
    let (s2c_client, _s2c_server) = duplex(SERVER_OUT_BUF);
    let (client_reader, _s2c_write) = split(s2c_client);
    let (_c2s_read, client_writer) = split(c2s_client);
    let client = Arc::new(Client::for_test(
        client_reader,
        client_writer,
        LifecycleHooks::default(),
    ));

    // Large REQUEST frame via the production roundtrip path: an 8 KiB
    // username makes envelope_bytes ≫ 1024.
    let req = DbRequest::SetReplicator {
        user: "u".repeat(8192),
        on: true,
        hmac: None,
    };
    let task_client = Arc::clone(&client);
    let task = tokio::spawn(async move { task_client.roundtrip(&req).await });

    // Synchronization point: exactly one byte consumed proves the write
    // started and is now blocked mid-frame (no sleeps).
    let mut first = [0u8; 1];
    tokio::time::timeout(Duration::from_secs(5), c2s_server.read_exact(&mut first))
        .await
        .expect("peer must observe the first mid-frame byte")
        .expect("read pipe");

    // Caller cancellation while the write is parked mid-frame.
    task.abort();
    assert!(task.await.is_err(), "task must have been cancelled");

    // The WriteCancelGuard's Drop must have closed the lifecycle: a FRESH
    // ping fails fast with the cancelled-write Io cause (never reuses the
    // corrupted stream).
    let err = tokio::time::timeout(Duration::from_secs(5), client.ping())
        .await
        .expect("ping must fail fast on the closed lifecycle, not hang")
        .expect_err("closed client must fail");
    assert_closed(
        &err,
        &CloseCause::Io(Arc::from("write cancelled mid-frame")),
    );
}

// ─── Write-phase: shutdown while the ping HOLDS the writer ─────────────

/// Parked at `after_writer_acquired` (writer HELD, recheck already passed),
/// the shutdown fully completes; on release the ping must resolve
/// `ConnectionClosed(PeerEof)` and write NOTHING — the biased
/// `closed_wait`-vs-`write_frame` race arm wins over the wire write.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rv_client_b42b8576_no_write_midframe_after_close() {
    let (hook, mut reached, release) = HookPoint::new();
    let hooks = LifecycleHooks {
        after_writer_acquired: Some(hook),
        ..Default::default()
    };
    let (mut client, mut ends) = setup(hooks);
    let reader = client.take_reader_handle().expect("reader handle");

    {
        let mut ping = pin!(client.ping());
        tokio::select! {
            r = &mut ping => panic!("ping resolved before release: {r:?}"),
            _ = &mut reached => {}
        }

        // Shutdown completes while the ping HOLDS the writer.
        drop(ends.server_out);
        reader.await.expect("reader_task panicked");

        release.send(()).expect("release after_writer_acquired");
        let err = ping.await.expect_err("ping must fail after close");
        assert_closed(&err, &CloseCause::PeerEof);
    }

    // Categorical: zero bytes may reach the wire after close.
    drop(client);
    assert_no_bytes_written(&mut ends.server_in).await;
}

// ─── RV-CLIENT-E16A04E5 — borrowed roundtrip envelope ──────────────────────

#[test]
fn rv_client_e16a04e5_roundtrip_serialize_uses_borrowed_request_bytes() {
    let source = include_str!("../../client/api.rs");
    let start = source
        .find("thread_local! {")
        .expect("scratch serializer exists");
    let end = source[start..]
        .find("let (tx, rx)")
        .expect("registration follows encoding");
    let serializer = &source[start..start + end];
    assert!(serializer.contains("RequestEnvelopeRef"));
    assert!(serializer.contains("req: &buf"));
    assert!(!serializer.contains("buf.clone()"));
}

/// Structural synchronization guard: scopes contain lock acquisition through
/// route/install operations; this is a source-shape guard, not a forced-interleaving proof.
#[test]
fn rv_client_b80c0d09_subscription_routing_install_share_mutex_structural_guard() {
    let transport = include_str!("../../client/transport.rs");
    let start = transport
        .find("let mut subs = state.subs.lock()")
        .expect("routing lock");
    let end = transport[start..]
        .find("if let PushAction::Live(res) = action")
        .expect("route")
        + start;
    let route = &transport[start..end];
    assert!(route.contains("subs.live.get(&envelope.sub)"));
    assert!(route.contains("subs.early.entry(envelope.sub)"));
    assert!(!route.contains("drop(subs)"));
    assert!(!route.contains("drop(guard)"));
    assert!(!route.contains("drop(_guard)"));
    let api = include_str!("../../client/api.rs");
    let start = api
        .find("let mut subs = self.state.subs.lock()")
        .expect("install lock");
    let end = api[start..].find("SubscriptionHandle::new").expect("flush") + start;
    let install = &api[start..end];
    assert!(install.contains("subs.live.insert(sub_id, tx.clone())"));
    assert!(install.contains("subs.early.remove(&sub_id)"));
    assert!(install.contains("subs.early_total -= buffered.len()"));
    assert!(install.contains("tx.try_send(envelope)"));
    assert!(!install.contains("drop(subs)"));
    assert!(!install.contains("drop(guard)"));
    assert!(!install.contains("drop(_guard)"));
}

/// Real wire flow uses response markers to order early and live pushes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rv_client_b80c0d09_real_subscription_fifo_flush_count_and_shutdown() {
    let (mut client, mut ends) = setup(LifecycleHooks::default());
    let reader = client.take_reader_handle().expect("reader handle");
    write_push(&mut ends.server_out, &push(17, 1)).await;
    let early_marker = async {
        let frame = read_frame(&mut ends.server_in, MAX_FRAME_SIZE_DEFAULT)
            .await
            .expect("ping frame");
        let request: shamir_connect::common::envelope::RequestEnvelope =
            rmp_serde::from_slice(&frame).expect("decode ping");
        write_response(
            &mut ends.server_out,
            request.request_id.expect("rid"),
            &pong_payload(),
        )
        .await;
    };
    let (ping_result, ()) = tokio::join!(client.ping(), early_marker);
    ping_result.expect("early marker response");
    let mut handle = client.subscribe_push(17);
    assert_eq!(
        client
            .lifecycle_for_test()
            .subs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .early_total,
        0
    );

    write_push(&mut ends.server_out, &push(17, 2)).await;
    let live_marker = async {
        let frame = read_frame(&mut ends.server_in, MAX_FRAME_SIZE_DEFAULT)
            .await
            .expect("second ping frame");
        let request: shamir_connect::common::envelope::RequestEnvelope =
            rmp_serde::from_slice(&frame).expect("decode second ping");
        write_response(
            &mut ends.server_out,
            request.request_id.expect("rid"),
            &pong_payload(),
        )
        .await;
    };
    let (ping_result, ()) = tokio::join!(client.ping(), live_marker);
    ping_result.expect("live marker response");
    assert_eq!(handle.next().await.expect("flushed early push").seq, 1);
    assert_eq!(handle.next().await.expect("live push").seq, 2);
    drop(ends.server_out);
    reader.await.expect("reader task");
    assert!(handle.next().await.is_none());
    assert_eq!(handle.termination(), Some(CloseCause::PeerEof));
    assert_eq!(
        client
            .lifecycle_for_test()
            .subs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .early_total,
        0
    );
}

/// Fill the aggregate early budget through the real client, then prove that
/// registration frees one slot for a new unknown subscription; shutdown clears it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rv_client_b80c0d09_real_early_budget_reuse_and_shutdown_count() {
    let (mut client, mut ends) = setup(LifecycleHooks::default());
    let reader = client.take_reader_handle().expect("reader handle");
    let cap = crate::client::subscription::EARLY_BUFFER_TOTAL_CAP;
    for sub in 0..cap as u64 {
        write_push(&mut ends.server_out, &push(sub, sub)).await;
    }
    let first_marker = async {
        let frame = read_frame(&mut ends.server_in, MAX_FRAME_SIZE_DEFAULT)
            .await
            .expect("ping frame");
        let req: shamir_connect::common::envelope::RequestEnvelope =
            rmp_serde::from_slice(&frame).expect("decode ping");
        write_response(
            &mut ends.server_out,
            req.request_id.expect("rid"),
            &pong_payload(),
        )
        .await;
    };
    let (ping_result, ()) = tokio::join!(client.ping(), first_marker);
    ping_result.expect("initial budget marker response");
    assert_eq!(
        client
            .lifecycle_for_test()
            .subs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .early_total,
        cap
    );

    let mut handle = client.subscribe_push(17);
    assert_eq!(
        client
            .lifecycle_for_test()
            .subs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .early_total,
        cap - 1
    );
    write_push(&mut ends.server_out, &push(cap as u64 + 7, 1)).await;
    let second_marker = async {
        let frame = read_frame(&mut ends.server_in, MAX_FRAME_SIZE_DEFAULT)
            .await
            .expect("ping frame");
        let req: shamir_connect::common::envelope::RequestEnvelope =
            rmp_serde::from_slice(&frame).expect("decode ping");
        write_response(
            &mut ends.server_out,
            req.request_id.expect("rid"),
            &pong_payload(),
        )
        .await;
    };
    let (ping_result, ()) = tokio::join!(client.ping(), second_marker);
    ping_result.expect("budget reuse marker response");
    assert_eq!(
        client
            .lifecycle_for_test()
            .subs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .early_total,
        cap
    );

    drop(ends.server_out);
    reader.await.expect("reader task");
    assert_eq!(handle.next().await.expect("buffered push drains").seq, 17);
    assert!(handle.next().await.is_none());
    assert_eq!(handle.termination(), Some(CloseCause::PeerEof));
    assert_eq!(
        client
            .lifecycle_for_test()
            .subs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .early_total,
        0
    );
}

/// Real roundtrip paused after fast check; shutdown completes before its
/// registration attempt, which must leave no pending entry or wire bytes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rv_client_b80c0d09_real_roundtrip_after_fast_check_closed_before_insert() {
    let (hook, mut reached, release) = HookPoint::new();
    let hooks = LifecycleHooks {
        after_fast_check: Some(hook),
        ..Default::default()
    };
    let (mut client, mut ends) = setup(hooks);
    let reader = client.take_reader_handle().expect("reader handle");
    {
        let mut ping = pin!(client.ping());
        tokio::select! { r = &mut ping => panic!("ping resolved before release: {r:?}"), _ = &mut reached => {} }
        drop(ends.server_out);
        reader.await.expect("reader task");
        release.send(()).expect("release fast check");
        assert_closed(&ping.await.expect_err("must close"), &CloseCause::PeerEof);
        assert!(client.lifecycle_for_test().pending.map.is_empty());
    }
    drop(client);
    assert_no_bytes_written(&mut ends.server_in).await;
}

/// Real roundtrip paused after pending registration; shutdown completes before release.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rv_client_b80c0d09_real_roundtrip_register_after_shutdown_fails_with_cause() {
    let (hook, mut reached, release) = HookPoint::new();
    let hooks = LifecycleHooks {
        after_register: Some(hook),
        ..Default::default()
    };
    let (mut client, mut ends) = setup(hooks);
    let reader = client.take_reader_handle().expect("reader handle");
    {
        let mut ping = pin!(client.ping());
        tokio::select! { r = &mut ping => panic!("ping resolved before release: {r:?}"), _ = &mut reached => {} }
        drop(ends.server_out);
        reader.await.expect("reader task");
        release.send(()).expect("release registration");
        assert_closed(&ping.await.expect_err("must close"), &CloseCause::PeerEof);
        assert!(client.lifecycle_for_test().pending.map.is_empty());
    }
    drop(client);
    assert_no_bytes_written(&mut ends.server_in).await;
}
