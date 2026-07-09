//! Duplex post-handshake request loop — M1.
//!
//! Architecture overview:
//!
//!   ┌──────────────┐       mpsc (cap=max_in_flight)       ┌─────────────┐
//!   │  Reader task │──── WriterMsg::{Reply,AndClose} ────►│ Writer task │
//!   │  (this task) │                                       │  (spawned)  │
//!   └──────────────┘                                       └─────────────┘
//!         │                                                       │
//!         │  Semaphore (max_in_flight permits)                    │
//!         │  JoinSet<()>  (one entry per in-flight request)       │
//!         └────────────────────────────────────────────────────────┘
//!
//! Back-pressure chain:
//!   1. Semaphore exhausted → reader blocks on `acquire_owned()` → no new
//!      frames read.
//!   2. Writer channel full → dispatch tasks block on `tx.send()` → permits
//!      held → semaphore exhausted → reader stalls.
//!
//! Reply ordering:
//!   Replies arrive in dispatch-completion order (not wire order). Clients
//!   must correlate by `request_id` (rid). `max_in_flight = 1` gives
//!   lock-step ordering identical to the old sequential loop.
//!
//! Teardown on any exit path:
//!   - `join_set.abort_all()` cancels in-flight dispatch tasks.
//!   - `tx` (Sender) is dropped, closing the channel.
//!   - Writer task sees channel closed → calls `writer.shutdown().await` and
//!     exits. `writer_handle.await` waits for that to complete.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Serialize;
use tokio::sync::{mpsc, Semaphore};
use tokio::task::JoinSet;

use shamir_connect::common::envelope::{ErrorEnvelope, RequestEnvelopeView};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::{dispatch_request_view, DispatchOutcome};
use shamir_connect::Error as ConnectError;

use super::push_sink::MpscPushSink;

use shamir_transport_tcp::framing::MAX_FRAME_SIZE_DEFAULT;

use crate::framer::{FrameReader, FrameWriter};
use crate::subscriptions::SubscriptionRegistry;

use super::connection_context::ConnectionContext;
use super::in_flight_guard::InFlightGuard;

/// Serialize `val` as named msgpack directly into a length-prefixed buffer,
/// avoiding the extra memcpy that `write_frame_into` does to prepend the
/// 4-byte length prefix.
///
/// Layout: `[4-byte BE u32 length][msgpack payload]`.
///
/// The caller MUST pass the result to `write_frame_prereserved` (not
/// `write_frame_into`).
fn encode_prereserved<T: Serialize>(val: &T) -> Result<Vec<u8>, rmp_serde::encode::Error> {
    let mut buf = Vec::with_capacity(256);
    // Reserve 4 bytes for the length prefix — patched after serialization.
    buf.extend_from_slice(&[0u8; 4]);
    rmp_serde::encode::write_named(&mut buf, val)?;
    let payload_len = (buf.len() - 4) as u32;
    buf[0..4].copy_from_slice(&payload_len.to_be_bytes());
    Ok(buf)
}

/// Wrap an already-serialized payload into a length-prefixed buffer.
pub(crate) fn prereserve_frame(payload: &[u8]) -> Vec<u8> {
    let len = payload.len() as u32;
    let mut buf = Vec::with_capacity(4 + payload.len());
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(payload);
    buf
}

/// Message sent from dispatch tasks to the writer task.
///
/// ALL variants carry a **prereserved** buffer: `[4-byte BE u32 length][payload]`.
/// The writer calls `write_frame_prereserved` directly — no internal memcpy
/// to prepend the length prefix (finding §3.4).
pub(crate) enum WriterMsg {
    /// Write these bytes and keep running.
    Reply(Vec<u8>),
    /// Write these bytes, then shut down the connection.
    ReplyAndClose(Vec<u8>),
    /// Server-initiated push frame (subscription event).
    Push(Vec<u8>),
}

/// Duplex post-handshake request loop for a single connection.
///
/// # Duplex model
///
/// After a successful SCRAM handshake the connection enters this loop which
/// drives reading and writing from two independent tasks:
///
/// * **Reader loop** (this task): reads frames from the client, acquires a
///   semaphore permit, and spawns a per-request dispatch task into a
///   `JoinSet`. Back-pressure: when `max_in_flight` permits are exhausted
///   the reader blocks and no new frames are accepted.
///
/// * **Writer task** (spawned): owns the write half of the framer. Receives
///   `WriterMsg::{Reply, ReplyAndClose}` over a bounded `mpsc` channel
///   (capacity = `max_in_flight`). Writes frames in receipt order; on
///   `ReplyAndClose` writes the final frame and shuts down.
///
/// * **Dispatch tasks** (one per request, in `JoinSet`): each owns the
///   frame bytes (fresh `Vec` — no per-connection buffer reuse on this
///   path), an `Arc<ConnectionContext>`, an `OwnedSemaphorePermit` (released
///   on task completion), and a clone of the `mpsc::Sender`. Each task
///   deserialises the envelope, runs `dispatch_request_view`, serialises
///   the response, and sends it to the writer.
///
/// # Reply ordering
///
/// Replies are written in dispatch-completion order, which is *not*
/// necessarily wire-arrival order. Clients must match responses to requests
/// by `request_id` (rid). Setting `max_in_flight = 1` reproduces the old
/// lock-step behaviour with strict ordering.
///
/// # Teardown
///
/// Any exit cause (client EOF, writer death, panic in dispatch) triggers:
/// 1. `join_set.abort_all()` — cancel in-flight tasks.
/// 2. Drop `tx` — signal writer task that no more messages are coming.
/// 3. `writer_handle.await` — wait for the writer to flush and shut down.
pub async fn request_loop<R, W>(
    ctx: Arc<ConnectionContext>,
    mut reader: R,
    writer: W,
    sid: [u8; 32],
) where
    R: FrameReader + 'static,
    W: FrameWriter + 'static,
{
    let cap = ctx.max_in_flight.max(1);
    let semaphore = Arc::new(Semaphore::new(cap));
    let (tx, mut rx) = mpsc::channel::<WriterMsg>(cap);

    // Build a ConnectionServices with a real push channel so subscription
    // bridges can send server-initiated frames to this connection's writer.
    let push_sink = Arc::new(MpscPushSink::new(tx.clone()));
    let registry = Arc::new(SubscriptionRegistry::new());
    let conn = Arc::new(ConnectionServices {
        conn_id: 0,
        push: Some(push_sink),
        extensions: Some(Arc::clone(&registry) as Arc<dyn std::any::Any + Send + Sync>),
    });

    // Shared flag: set to true by a dispatch task that sends ReplyAndClose.
    // The reader loop checks this flag and stops accepting new frames.
    let close_requested = Arc::new(AtomicBool::new(false));

    // --- Writer task ---------------------------------------------------------
    // Owns the write half; receives replies over mpsc; shuts down on
    // channel-close or ReplyAndClose. §B21: JoinHandle is always awaited.
    //
    // All WriterMsg payloads are prereserved ([4-byte len][payload]) — the
    // writer calls `write_frame_prereserved` directly, skipping the memcpy
    // that `write_frame_into` does to prepend the length prefix (§3.4).
    let mut writer_handle = tokio::spawn(async move {
        let mut writer = writer;
        loop {
            match rx.recv().await {
                None => {
                    // Channel closed (all senders dropped) — clean exit.
                    break;
                }
                Some(WriterMsg::Reply(bytes)) => {
                    // DEFECT C fix: on write error (broken pipe / dead client)
                    // break immediately so the JoinHandle resolves and the
                    // reader's select! branch wakes up (Defect B fix).
                    if writer.write_frame_prereserved(&bytes).await.is_err() {
                        break;
                    }
                }
                Some(WriterMsg::Push(bytes)) => {
                    if writer.write_frame_prereserved(&bytes).await.is_err() {
                        break;
                    }
                }
                Some(WriterMsg::ReplyAndClose(bytes)) => {
                    // Write error here is ignored — we're closing anyway.
                    let _ = writer.write_frame_prereserved(&bytes).await;
                    break;
                }
            }
        }
        writer.shutdown().await;
    });

    let mut join_set: JoinSet<()> = JoinSet::new();

    // Tracks whether the writer task has already been consumed by the select!
    // branch below so teardown does not double-await it.
    let mut writer_done = false;

    // --- Reader loop ---------------------------------------------------------
    // Acquire permit → read frame → spawn dispatch.
    //
    // `read_frame_into` is placed inside `tokio::select!` with a branch that
    // watches the writer task handle. Cancel-safety of the read branch is
    // intentionally NOT required here: when the writer branch fires we are
    // tearing down the connection entirely, so any partially-read frame is
    // discarded along with everything else. We never resume the read after
    // the writer exits.
    'conn: loop {
        // Non-blocking drain of completed dispatch tasks: releases permits
        // and surfaces panics before we block on the next acquire.
        while let Some(result) = join_set.try_join_next() {
            if let Err(e) = result {
                if e.is_panic() {
                    tracing::error!("dispatch task panicked: {:?}", e);
                    // DEFECT A fix: a dispatch panic is fatal for this
                    // connection. Use the labeled break to exit the outer
                    // 'conn loop, not just this inner while.
                    break 'conn;
                }
            }
        }

        // Check the ReplyAndClose flag set by a dispatch task.
        if close_requested.load(Ordering::Relaxed) {
            break;
        }

        // Acquire a semaphore permit (back-pressure gate).
        // When all max_in_flight slots are taken this awaits the release of
        // an existing permit by a completing dispatch task.
        let permit = match semaphore.clone().acquire_owned().await {
            Ok(p) => p,
            Err(_) => break, // semaphore closed — should never happen
        };

        // Double-check the close flag: a ReplyAndClose dispatch task may have
        // completed while we were waiting for the permit.
        if close_requested.load(Ordering::Relaxed) {
            drop(permit);
            break;
        }

        // Read the next frame. DEFECT B fix: run inside select! so that if
        // the writer task exits (e.g. after ReplyAndClose or a write error)
        // the reader does not block forever waiting for data from a client
        // that is intentionally holding the TCP connection open.
        // Cancel-safety of the read branch is not required — when the writer
        // branch fires we discard the partial read and tear down immediately.
        let mut frame_buf = Vec::new();
        tokio::select! {
            read_res = reader.read_frame_into(MAX_FRAME_SIZE_DEFAULT, &mut frame_buf) => {
                match read_res {
                    Ok(()) => {}
                    Err(_) => {
                        // Client closed or transport error.
                        drop(permit);
                        break;
                    }
                }
            }
            _ = &mut writer_handle => {
                // Writer task exited (ReplyAndClose sent, or write error).
                // Tear down immediately; do not block on a lingering client.
                drop(permit);
                writer_done = true;
                break;
            }
        }

        // Spawn a per-request dispatch task. Each task owns:
        //   - `frame_buf`: raw msgpack bytes (fresh Vec — no reuse on
        //     concurrent path)
        //   - `ctx_clone`: Arc — cheap clone, shared read-only state
        //   - `permit`: OwnedSemaphorePermit — released when task ends
        //   - `tx_clone`: mpsc Sender — pushes reply to writer task
        //   - `close_flag`: signals ReplyAndClose to the reader loop
        let ctx_clone = Arc::clone(&ctx);
        let tx_clone = tx.clone();
        let close_flag = Arc::clone(&close_requested);
        let sid_copy = sid;
        let conn_clone = Arc::clone(&conn);

        join_set.spawn(async move {
            let _guard = InFlightGuard::new();
            let _permit = permit;

            let msg = match RequestEnvelopeView::from_msgpack(&frame_buf) {
                Ok(v) => {
                    let lookup_tib = |uid: &[u8; 16]| -> u64 {
                        ctx_clone.user_dir.tickets_invalid_before_ns_by_user_id(uid)
                    };
                    match dispatch_request_view(
                        &v,
                        &ctx_clone.session_store,
                        lookup_tib,
                        ctx_clone.handler.as_ref(),
                        &conn_clone,
                    )
                    .await
                    {
                        Ok(DispatchOutcome::Response(resp)) => {
                            let rid = v.request_id;
                            // §3.4: serialize directly into a length-prefixed
                            // buffer to avoid the memcpy in write_frame_into.
                            match encode_prereserved(&resp) {
                                Ok(b) => Some(WriterMsg::Reply(b)),
                                Err(_) => {
                                    // Serialisation failure — best-effort error.
                                    let err = ErrorEnvelope::new(rid, "internal_error");
                                    encode_prereserved(&err).ok().map(WriterMsg::Reply)
                                }
                            }
                        }
                        Ok(DispatchOutcome::Error(err)) => {
                            let close = err.error == "session_invalidated"
                                || err.error == "session_expired";
                            match encode_prereserved(&err) {
                                Ok(b) => {
                                    if close {
                                        // Signal the reader loop to stop.
                                        close_flag.store(true, Ordering::Relaxed);
                                        Some(WriterMsg::ReplyAndClose(b))
                                    } else {
                                        Some(WriterMsg::Reply(b))
                                    }
                                }
                                Err(_) => None,
                            }
                        }
                        Err(_) => {
                            // Internal dispatch error.
                            let err = ErrorEnvelope::new(v.request_id, "internal_error");
                            encode_prereserved(&err).ok().map(WriterMsg::Reply)
                        }
                    }
                }
                Err(_) => {
                    // Malformed envelope.
                    let err = ErrorEnvelope::new(None, "invalid_envelope");
                    encode_prereserved(&err).ok().map(WriterMsg::Reply)
                }
            };

            if let Some(msg) = msg {
                // `send` provides back-pressure: blocks when the channel
                // is at capacity. A slow writer stalls dispatch tasks →
                // permits held → semaphore exhausted → reader stalls. §B14.
                let _ = tx_clone.send(msg).await;
            }

            let _ = sid_copy;
        });
    }

    // --- Teardown ------------------------------------------------------------
    // Cancel all active subscriptions (aborts bridge tasks that hold
    // Arc<MpscPushSink> clones — must happen before dropping conn/tx).
    registry.close_all();
    // Cancel all in-flight dispatch tasks (they hold Arc<conn> + tx clones).
    join_set.abort_all();
    // Drain the JoinSet so aborted tasks drop their Arc<conn>/tx clones.
    while join_set.join_next().await.is_some() {}
    // Drop conn (holds Arc<MpscPushSink> → tx.clone()) so the writer
    // channel can close once all senders are gone.
    drop(conn);
    // Dropping tx closes the mpsc channel; the writer task sees None on its
    // next recv() and exits gracefully after flushing what it has.
    drop(tx);
    // Wait for the writer task to finish. §B21: no detached tasks.
    // If writer_done is true the select! branch already consumed the handle.
    if !writer_done {
        let _ = writer_handle.await;
    }

    let _ = sid;
    let _ = ConnectError::AuthFailed;
}
