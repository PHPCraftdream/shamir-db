//! Transport plumbing for the client: the transport-agnostic write sink,
//! frame decoding, the bounded connect helpers and the rid-demux background
//! reader task.

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::task::{Context, Poll};

use serde::{Deserialize, Serialize};
use shamir_collections::TFxMap;
use tokio::io::{AsyncWrite, WriteHalf};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio_rustls::client::TlsStream;

use shamir_transport_ipc::IpcClientStream;

use shamir_connect::common::envelope::{ErrorEnvelope, ResponseEnvelope};
use shamir_connect::common::push_envelope::PushEnvelope;

use shamir_transport_tcp::framing::{read_frame_into, FrameError, MAX_FRAME_SIZE_DEFAULT};

use crate::client::subscription::{EarlyBuffer, SubscriptionMap, EARLY_BUFFER_CAP};
use crate::error::ClientError;

/// [`Client`]'s write half, over whichever transport it connected with.
/// `Client` itself stays a single concrete type regardless of transport —
/// only this enum (and the analogous, but unnamed, generic `R` in
/// `reader_task`) varies. `AsyncWrite` is implemented by delegation to
/// the active variant; every existing call site (`write_frame(&mut
/// *guard, ...)`, `AsyncWriteExt::shutdown(&mut *guard)`) is already
/// generic over `W: AsyncWrite`, so none of them needed to change.
pub(crate) enum WriteSink {
    Tls(WriteHalf<TlsStream<TcpStream>>),
    Ipc(WriteHalf<IpcClientStream>),
}

impl AsyncWrite for WriteSink {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            WriteSink::Tls(w) => Pin::new(w).poll_write(cx, buf),
            WriteSink::Ipc(w) => Pin::new(w).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            WriteSink::Tls(w) => Pin::new(w).poll_flush(cx),
            WriteSink::Ipc(w) => Pin::new(w).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            WriteSink::Tls(w) => Pin::new(w).poll_shutdown(cx),
            WriteSink::Ipc(w) => Pin::new(w).poll_shutdown(cx),
        }
    }
}

/// Result of a demux decode: either a response payload or a transport-level
/// error string, both tagged with the correlation id.
enum DemuxResult {
    /// Success envelope: `(rid, payload_bytes)`.
    Response { rid: Option<u32>, payload: Vec<u8> },
    /// Error envelope: `(rid, error_string)`.
    Error { rid: Option<u32>, error: String },
}

/// Decode one raw frame into a [`DemuxResult`].
///
/// Strategy: try `ResponseEnvelope` first (contains `res` bytes field);
/// on failure try `ErrorEnvelope` (contains `error` string field).
/// If both fail, return `None` so the reader task can log-and-drop.
fn decode_frame(buf: &[u8]) -> Option<DemuxResult> {
    if let Ok(env) = ResponseEnvelope::from_msgpack(buf) {
        return Some(DemuxResult::Response {
            rid: env.request_id,
            payload: env.res,
        });
    }
    if let Ok(env) = ErrorEnvelope::from_msgpack(buf) {
        return Some(DemuxResult::Error {
            rid: env.request_id,
            error: env.error,
        });
    }
    None
}

/// Establish the underlying TCP connection, optionally bounded by a
/// connect deadline.
///
/// `timeout` is `None` → unbounded wait (the OS TCP stack is the only
/// bound), preserving the client's original behaviour for callers that do
/// not set [`ConnectOptions::connect_timeout`]. `Some(d)` bounds the
/// attempt via [`tokio::time::timeout`]; if the socket does not connect
/// within `d`, [`ClientError::ConnectTimeout`] is returned.
pub(crate) async fn connect_tcp(
    addr: SocketAddr,
    timeout: Option<std::time::Duration>,
) -> Result<TcpStream, ClientError> {
    match timeout {
        None => Ok(TcpStream::connect(addr).await?),
        Some(d) => match tokio::time::timeout(d, TcpStream::connect(addr)).await {
            Ok(res) => Ok(res?),
            Err(_elapsed) => Err(ClientError::ConnectTimeout(d)),
        },
    }
}

/// Establish the underlying local-IPC connection (Unix domain socket /
/// Windows Named Pipe), optionally bounded by a connect deadline. Mirrors
/// [`connect_tcp`] — see its docs for the `timeout` semantics.
pub(crate) async fn connect_ipc(
    addr: &str,
    timeout: Option<std::time::Duration>,
) -> Result<IpcClientStream, ClientError> {
    match timeout {
        None => Ok(shamir_transport_ipc::connect(addr).await?),
        Some(d) => match tokio::time::timeout(d, shamir_transport_ipc::connect(addr)).await {
            Ok(res) => Ok(res?),
            Err(_elapsed) => Err(ClientError::ConnectTimeout(d)),
        },
    }
}

/// Per-pending-request slot: either resolved with response bytes or with a
/// transport-level error.
pub(crate) type PendingSender = oneshot::Sender<Result<Vec<u8>, ClientError>>;
pub(crate) type PendingMap = Arc<StdMutex<TFxMap<u32, PendingSender>>>;

/// Await the reader task's response for `rid`, optionally bounded by a
/// per-request deadline.
///
/// - `timeout == None` → unbounded await (the original behaviour): resolve
///   only when the reader task delivers the payload, a transport error, or
///   drops the sender (connection closed).
/// - `timeout == Some(d)` → bound the await via [`tokio::time::timeout`]. On
///   elapse, the now-orphaned pending entry for `rid` is removed (so a late
///   response is cleanly dropped instead of leaking a dead sender) and
///   [`ClientError::RequestTimeout`] is returned.
///
/// Collapses the three success/error outcomes into a single
/// `Result<Vec<u8>, ClientError>`: `Ok(payload)` on success, the reader
/// task's `ClientError` on a delivered error, `ConnectionClosed` if the
/// sender was dropped, or `RequestTimeout` on deadline elapse.
pub(crate) async fn await_pending_response(
    rx: oneshot::Receiver<Result<Vec<u8>, ClientError>>,
    timeout: Option<std::time::Duration>,
    pending: &PendingMap,
    rid: u32,
) -> Result<Vec<u8>, ClientError> {
    let recv = match timeout {
        None => rx.await,
        Some(d) => match tokio::time::timeout(d, rx).await {
            Ok(res) => res,
            Err(_elapsed) => {
                // Drop the now-orphaned pending entry so a late response for
                // this rid is cleanly dropped instead of leaking a dead
                // sender in the map.
                let mut map = pending.lock().unwrap_or_else(|p| p.into_inner());
                map.remove(&rid);
                return Err(ClientError::RequestTimeout(d));
            }
        },
    };
    match recv {
        Ok(inner) => inner,
        // oneshot sender was dropped — reader task died.
        Err(_) => Err(ClientError::ConnectionClosed),
    }
}

/// Background reader loop.  Owns `ReadHalf`; demuxes frames to pending waiters.
///
/// On EOF or I/O error: marks `closed`, drains `pending` (sends
/// `ConnectionClosed` to every waiter), then exits.
pub(crate) async fn reader_task<R>(
    mut reader: R,
    pending: PendingMap,
    closed: Arc<AtomicBool>,
    subscriptions: SubscriptionMap,
    early_buffer: EarlyBuffer,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    // T-tcp-1: reuse a single buffer across frames — avoids per-frame heap
    // allocation. Capacity grows to the high-water mark and is never freed
    // until the task exits. Borrow is fully released before every `.await`
    // below that is not `read_frame_into` itself, so no lifetime crosses an
    // unrelated await point.
    let mut frame_buf: Vec<u8> = Vec::with_capacity(4096);
    loop {
        match read_frame_into(&mut reader, MAX_FRAME_SIZE_DEFAULT, &mut frame_buf).await {
            Ok(()) => {}
            Err(FrameError::PeerClose) => {
                // Graceful close from server.
                break;
            }
            Err(e) => {
                tracing::debug!("reader_task: read_frame error: {e}");
                break;
            }
        }
        let result = match decode_frame(&frame_buf) {
            Some(r) => r,
            None => {
                // Not a response/error envelope — try push frame.
                if let Ok(envelope) = rmp_serde::from_slice::<PushEnvelope>(&frame_buf) {
                    let sender = {
                        let map = subscriptions.lock().unwrap_or_else(|p| p.into_inner());
                        map.get(&envelope.sub).cloned()
                    };
                    if let Some(tx) = sender {
                        match tx.try_send(envelope) {
                            Ok(()) => {}
                            Err(tokio::sync::mpsc::error::TrySendError::Full(env)) => {
                                tracing::warn!(
                                    "client mpsc full for sub={}; dropping push",
                                    env.sub
                                );
                            }
                            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                                // Consumer dropped — SubscriptionHandle::Drop has or
                                // will clean up the registry entry.
                            }
                        }
                    } else {
                        let mut buf = early_buffer.lock().unwrap_or_else(|p| p.into_inner());
                        let entry = buf.entry(envelope.sub).or_default();
                        if entry.len() < EARLY_BUFFER_CAP {
                            entry.push(envelope);
                        } else {
                            tracing::debug!(
                                "reader_task: early buffer full for sub={}, dropping",
                                envelope.sub
                            );
                        }
                    }
                } else {
                    tracing::debug!(
                        "reader_task: could not decode frame ({} bytes), dropping",
                        frame_buf.len()
                    );
                }
                continue;
            }
        };

        let (rid, outcome) = match result {
            DemuxResult::Response { rid, payload } => (rid, Ok(payload)),
            DemuxResult::Error { rid, error } => (
                rid,
                Err(ClientError::Protocol(format!(
                    "server error envelope: {error}"
                ))),
            ),
        };

        let rid = match rid {
            Some(r) => r,
            None => {
                // Response/error envelope without rid — unusual but harmless.
                tracing::debug!("reader_task: frame without rid, dropping");
                continue;
            }
        };

        let sender = {
            // SAFETY of lock: std::sync::Mutex, no .await while held.
            let mut map = pending.lock().unwrap_or_else(|p| p.into_inner());
            map.remove(&rid)
        };

        match sender {
            Some(tx) => {
                // Ignore the error: the waiter may have been cancelled.
                let _ = tx.send(outcome);
            }
            None => {
                // Late response for a cancelled/timed-out request.
                tracing::debug!("reader_task: no waiter for rid={rid}, dropping");
            }
        }
    }

    // Connection is dead — mark closed and drain pending map.
    closed.store(true, Ordering::Release);
    let waiters: Vec<PendingSender> = {
        let mut map = pending.lock().unwrap_or_else(|p| p.into_inner());
        map.drain().map(|(_, tx)| tx).collect()
    };
    for tx in waiters {
        let _ = tx.send(Err(ClientError::ConnectionClosed));
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct WireAuthInit {
    pub user: String,
    #[serde(with = "serde_bytes")]
    pub client_nonce: Vec<u8>,
    pub binding_mode: u8,
    pub version: u8,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct WireChallenge {
    #[serde(with = "serde_bytes")]
    pub salt: Vec<u8>,
    pub memory_kb: u32,
    pub time: u32,
    pub parallelism: u32,
    pub argon2_version: u8,
    #[serde(with = "serde_bytes")]
    pub server_nonce: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct WireClientProof {
    #[serde(with = "serde_bytes")]
    pub client_proof: Vec<u8>,
}

/// Sent by the client as the first frame when resuming a session via ticket.
#[derive(Serialize)]
pub(crate) struct WireResumeInit {
    #[serde(with = "serde_bytes")]
    pub ticket: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub client_nonce: Vec<u8>,
    pub binding_mode: u8,
}

/// Server response to a successful [`WireResumeInit`].
#[derive(Deserialize)]
pub(crate) struct WireResumeOk {
    #[serde(with = "serde_bytes")]
    pub session_id: Vec<u8>,
    pub expires_at_ns: u64,
    #[serde(default, with = "serde_bytes")]
    pub resumption_ticket: Vec<u8>,
    #[serde(default)]
    pub resumption_expires_at_ns: u64,
    /// Max query-language version this server supports; `0` when absent
    /// (old server). Client emits v2 id-keyed write/read only when >= 2.
    #[serde(default)]
    pub server_query_version: u8,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct WireAuthOk {
    #[serde(with = "serde_bytes")]
    pub server_signature: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub server_pub_key: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub identity_sig: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub session_id: Vec<u8>,
    pub expires_at_ns: u64,
    /// Resumption ticket — empty `Vec` if the server didn't issue one.
    #[serde(default, with = "serde_bytes")]
    pub resumption_ticket: Vec<u8>,
    #[serde(default)]
    pub resumption_expires_at_ns: u64,
    /// Max query-language version this server supports; `0` when absent
    /// (old server). Client emits v2 id-keyed write/read only when >= 2.
    #[serde(default)]
    pub server_query_version: u8,
}
