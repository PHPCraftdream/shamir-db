//! Transport plumbing for the client: the transport-agnostic write sink,
//! frame decoding, the bounded connect helpers and the rid-demux background
//! reader task.

use std::cell::Cell;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::task::{Context, Poll};

use serde::{Deserialize, Serialize};
use shamir_collections::{TFxMap, THasher};
use tokio::io::{AsyncWrite, WriteHalf};
use tokio::net::TcpStream;
#[cfg(test)]
use tokio::sync::mpsc;
use tokio::sync::{oneshot, watch};
use tokio_rustls::client::TlsStream;

use shamir_transport_ipc::IpcClientStream;

use shamir_connect::common::envelope::{ErrorEnvelope, ResponseEnvelope};
use shamir_connect::common::push_envelope::PushEnvelope;

use shamir_transport_tcp::framing::{read_frame_into, FrameError, MAX_FRAME_SIZE_DEFAULT};

use crate::client::subscription::{PushSender, EARLY_BUFFER_CAP, EARLY_BUFFER_TOTAL_CAP};
use crate::error::{ClientError, CloseCause};

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
    /// Test-only sink: the client→server half of an injected
    /// simplex/duplex pipe (P0 C1 `Client::for_test`).
    #[cfg(test)]
    Test(Box<dyn AsyncWrite + Send + Unpin>),
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
            #[cfg(test)]
            WriteSink::Test(w) => Pin::new(&mut **w).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            WriteSink::Tls(w) => Pin::new(w).poll_flush(cx),
            WriteSink::Ipc(w) => Pin::new(w).poll_flush(cx),
            #[cfg(test)]
            WriteSink::Test(w) => Pin::new(&mut **w).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            WriteSink::Tls(w) => Pin::new(w).poll_shutdown(cx),
            WriteSink::Ipc(w) => Pin::new(w).poll_shutdown(cx),
            #[cfg(test)]
            WriteSink::Test(w) => Pin::new(&mut **w).poll_shutdown(cx),
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

/// Monotonic close state (RV-CLIENT-B80C0D09): the cause is stored before
/// the flag is raised, so Acquire readers observing the flag see the cause.
#[derive(Default)]
pub(crate) struct ClosedState {
    flag: AtomicBool,
    cause: OnceLock<CloseCause>,
}

impl ClosedState {
    /// Publish the terminal cause. Returns `true` only for the FIRST caller
    /// (all later causes are dropped — first cause wins), making exactly one
    /// [`Lifecycle::shutdown`] invocation the single drainer of both
    /// registries.
    pub(crate) fn publish(&self, cause: CloseCause) -> bool {
        if self.cause.set(cause).is_err() {
            return false;
        }
        self.flag.store(true, Ordering::Release);
        true
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }

    /// The stored cause; `Some` iff closed. The `Acquire` flag load pairs
    /// with the `Release` flag store that follows `OnceLock::set`, so the
    /// `get` below always finds the cause once `is_closed()` held.
    pub(crate) fn cause(&self) -> Option<CloseCause> {
        if !self.is_closed() {
            return None;
        }
        Some(
            self.cause
                .get()
                .cloned()
                .unwrap_or(CloseCause::ReaderCancelled),
        )
    }
}

/// Pending-request registry: request id → oneshot sender. `scc::HashMap` uses
/// internal bucket synchronization. Close-vs-register protocol: the cause is
/// published BEFORE the drain, so a registrant re-reads
/// `closed.cause()` AFTER a successful insert — an insert that landed before
/// the drain is resolved by it; one that lands after removes its own entry
/// and fails with the published cause.
pub(crate) struct PendingRegistry {
    pub(crate) closed: ClosedState,
    pub(crate) map: scc::HashMap<u32, PendingSender, THasher>,
}

/// Pending-request registry handle.
pub(crate) type PendingMap = Arc<PendingRegistry>;

/// Standard mutex guards the coherent subscription state; its contention model is one reader task plus rare subscribe calls per connection.
pub(crate) struct SubsInner {
    pub(crate) closed: Option<CloseCause>,
    pub(crate) live: TFxMap<u64, PushSender>,
    pub(crate) early: TFxMap<u64, Vec<PushEnvelope>>,
    pub(crate) early_total: usize,
    pub(crate) terminal: TFxMap<u64, oneshot::Sender<CloseCause>>,
}

pub(crate) type SubsMap = Arc<StdMutex<SubsInner>>;

/// The ONE closeable lifecycle state shared by request registration,
/// subscription registration and teardown (P0 C1). `closed_hint` is a FAST
/// PATH ONLY (not authoritative); the authoritative lifecycle cause is in PendingRegistry.
#[derive(Clone)]
pub(crate) struct Lifecycle {
    pub(crate) pending: PendingMap,
    pub(crate) subs: SubsMap,
    /// FAST PATH ONLY hint raised by [`Lifecycle::shutdown`]. The
    /// authoritative state lives under the registry locks.
    pub(crate) closed_hint: Arc<AtomicBool>,
    /// Closure broadcast (P0 write-phase race): `Some(cause)` once shutdown
    /// ran. Lets waiters blocked OUTSIDE the registry locks (writer
    /// acquisition, in-flight `write_frame`) unblock on the terminal
    /// transition.
    pub(crate) closed_tx: Arc<watch::Sender<Option<CloseCause>>>,
}

impl Lifecycle {
    pub(crate) fn new() -> Self {
        Self {
            pending: Arc::new(PendingRegistry {
                closed: ClosedState::default(),
                map: scc::HashMap::with_hasher(THasher::default()),
            }),
            subs: Arc::new(StdMutex::new(SubsInner {
                closed: None,
                live: TFxMap::default(),
                early: TFxMap::default(),
                early_total: 0,
                terminal: TFxMap::default(),
            })),
            closed_hint: Arc::new(AtomicBool::new(false)),
            closed_tx: Arc::new(watch::Sender::new(None)),
        }
    }

    /// The stored close cause, if the lifecycle is closed.
    pub(crate) fn cause(&self) -> Option<CloseCause> {
        self.pending.closed.cause()
    }

    /// Resolves with the close cause once [`Lifecycle::shutdown`] has run.
    /// Used to race blocking phases of `roundtrip` (writer acquisition,
    /// `write_frame`) against closure.
    pub(crate) async fn closed_wait(&self) -> CloseCause {
        let mut rx = self.closed_tx.subscribe();
        // `shutdown` publishes via `send_replace` (stored unconditionally), so
        // the cause may ALREADY be here if shutdown ran before we subscribed —
        // check synchronously first to avoid a lost wakeup.
        if let Some(cause) = rx.borrow_and_update().clone() {
            return cause;
        }
        let cause = match rx.wait_for(|c| c.is_some()).await {
            Ok(cause) => cause.clone().unwrap_or(CloseCause::ReaderCancelled),
            // Sender gone: every Lifecycle clone dropped — nothing can close.
            Err(_) => CloseCause::ReaderCancelled,
        };
        cause
    }

    /// Idempotent terminal transition; the FIRST cause wins. Subscription
    /// shutdown sends each terminal cause, then clears route/buffer state,
    /// under the same mutex as registration and reader routing.
    pub(crate) fn shutdown(&self, cause: CloseCause) {
        if !self.pending.closed.publish(cause.clone()) {
            // Already closed — first cause wins, nothing to do.
            return;
        }
        {
            let mut subs = self.subs.lock().unwrap_or_else(|p| p.into_inner());
            if subs.closed.is_some() {
                return;
            }
            subs.closed = Some(cause.clone());
            for tx in subs.terminal.drain().map(|(_, tx)| tx) {
                let _ = tx.send(cause.clone());
            }
            debug_assert!(subs.closed.is_some());
            subs.live.clear();
            subs.early.clear();
            subs.early_total = 0;
            debug_assert_eq!(
                subs.early_total,
                subs.early.values().map(Vec::len).sum::<usize>()
            );
        }
        self.closed_tx.send_replace(Some(cause.clone()));
        self.closed_hint.store(true, Ordering::Release);
        let mut waiters: Vec<PendingSender> = Vec::new();
        self.pending.map.retain_sync(|_, tx| {
            // Move the sender out (oneshot::Sender is not Clone): leave a
            // fresh throwaway channel behind — the entry is removed anyway.
            let (dummy, _) = oneshot::channel();
            waiters.push(std::mem::replace(tx, dummy));
            false
        });
        for tx in waiters {
            let _ = tx.send(Err(ClientError::ConnectionClosed(cause.clone())));
        }
    }
}

/// Drop guard installed by [`reader_task`] BEFORE its read loop. On any exit
/// path that did not set a cause explicitly (EOF / I/O error do) — abort,
/// cancel, panic — it shuts the lifecycle down with
/// [`CloseCause::ReaderCancelled`].
pub(crate) struct ShutdownGuard {
    state: Lifecycle,
    cause: Cell<Option<CloseCause>>,
}

impl ShutdownGuard {
    pub(crate) fn new(state: Lifecycle) -> Self {
        Self {
            state,
            cause: Cell::new(Some(CloseCause::ReaderCancelled)),
        }
    }

    pub(crate) fn set(&self, cause: CloseCause) {
        self.cause.set(Some(cause));
    }
}

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        let cause = self.cause.take().unwrap_or(CloseCause::ReaderCancelled);
        self.state.shutdown(cause);
    }
}

/// One-shot test hook point: production code FIRES it (sends `reached`, then
/// awaits `release`); the test awaits `reached` and holds `release`.
#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct HookPoint {
    reached: Arc<StdMutex<Option<oneshot::Sender<()>>>>,
    release: Arc<StdMutex<Option<oneshot::Receiver<()>>>>,
    /// Multi-fire channel (see [`HookPoint::new_multi`]).
    multi: Option<mpsc::UnboundedSender<()>>,
}

#[cfg(test)]
impl HookPoint {
    /// Multi-fire variant (test oracle quality): every `fire()` sends a
    /// `reached` signal and NEVER blocks (release side unarmed). Returns
    /// `(point, reached_rx)`; the test awaits exactly `expected_fires`
    /// signals.
    pub(crate) fn new_multi(expected_fires: usize) -> (Self, mpsc::UnboundedReceiver<()>) {
        let (tx, rx) = mpsc::unbounded_channel();
        debug_assert!(expected_fires > 0, "arm with a positive fire count");
        (
            Self {
                reached: Arc::new(StdMutex::new(None)),
                release: Arc::new(StdMutex::new(None)),
                multi: Some(tx),
            },
            rx,
        )
    }

    /// Create an armed hook: `(point, reached_rx, release_tx)`.
    pub(crate) fn new() -> (Self, oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (reached_tx, reached_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        (
            Self {
                reached: Arc::new(StdMutex::new(Some(reached_tx))),
                release: Arc::new(StdMutex::new(Some(release_rx))),
                multi: None,
            },
            reached_rx,
            release_tx,
        )
    }

    /// Fired by the production path: signal `reached`, then block until the
    /// test releases (or drops `release_tx`). Never blocks when unarmed or
    /// in multi mode.
    pub(crate) async fn fire(&self) {
        if let Some(tx) = self
            .reached
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            let _ = tx.send(());
        }
        if let Some(multi) = &self.multi {
            let _ = multi.send(());
        }
        // Take the receiver OUT of the lock before awaiting: the guard must
        // never be held across the `.await`.
        let rx = self
            .release
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        if let Some(rx) = rx {
            let _ = rx.await;
        }
    }
}

/// Test-only lifecycle seam (P0 C1): pause points fired BY the production
/// code paths. The TYPE exists in both builds (production call sites pass
/// `LifecycleHooks::default()`), but every FIELD — and [`HookPoint`] — is
/// `#[cfg(test)]`, so nothing hook-related compiles into production builds.
#[derive(Clone, Default)]
pub(crate) struct LifecycleHooks {
    /// Reader loop ended, before the terminal shutdown transition.
    #[cfg(test)]
    pub(crate) reader_before_shutdown: Option<HookPoint>,
    /// `roundtrip` passed the fast-path closed check.
    #[cfg(test)]
    pub(crate) after_fast_check: Option<HookPoint>,
    /// `roundtrip` registered its pending sender under the registry lock.
    #[cfg(test)]
    pub(crate) after_register: Option<HookPoint>,
    /// `roundtrip` acquired the writer and passed the authoritative recheck.
    #[cfg(test)]
    pub(crate) after_writer_acquired: Option<HookPoint>,
}

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
///
/// The receive is raced against [`Lifecycle::closed_wait`]
/// (RV-CLIENT-B80C0D09, belt-and-suspenders): even a hypothetically
/// stranded pending entry can never hang the caller — closure unblocks it
/// and the orphaned entry is removed.
pub(crate) async fn await_pending_response(
    rx: oneshot::Receiver<Result<Vec<u8>, ClientError>>,
    timeout: Option<std::time::Duration>,
    state: &Lifecycle,
    rid: u32,
) -> Result<Vec<u8>, ClientError> {
    let recv = match timeout {
        None => tokio::select! {
            biased;
            r = rx => r,
            cause = state.closed_wait() => {
                state.pending.map.remove_sync(&rid);
                return Err(ClientError::ConnectionClosed(cause));
            }
        },
        Some(d) => tokio::select! {
            biased;
            r = tokio::time::timeout(d, rx) => match r {
                Ok(res) => res,
                Err(_elapsed) => {
                    state.pending.map.remove_sync(&rid);
                    return Err(ClientError::RequestTimeout(d));
                }
            },
            cause = state.closed_wait() => {
                state.pending.map.remove_sync(&rid);
                return Err(ClientError::ConnectionClosed(cause));
            }
        },
    };
    match recv {
        Ok(inner) => inner,
        // oneshot sender was dropped — reader task died.
        Err(_) => Err(ClientError::ConnectionClosed(
            state.cause().unwrap_or(CloseCause::ReaderCancelled),
        )),
    }
}

/// Background reader loop.  Owns the read half; demuxes frames to pending
/// waiters and live/early subscription channels via the coherent subscription mutex.
///
/// A [`ShutdownGuard`] is installed BEFORE the loop: EOF and I/O errors set
/// their cause explicitly; abort/cancel/panic fall back to
/// [`CloseCause::ReaderCancelled`]. The guard's `Drop` runs the idempotent
/// [`Lifecycle::shutdown`], so EVERY exit path closes subscriptions and
/// drains pending requests.
pub(crate) async fn reader_task<R>(
    mut reader: R,
    state: Lifecycle,
    guard: ShutdownGuard,
    hooks: LifecycleHooks,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    // T-tcp-1: reuse a single buffer across frames — avoids per-frame heap
    // allocation. Capacity grows to the high-water mark and is never freed
    // until the task exits. Borrow is fully released before every `.await`
    // below that is not `read_frame_into` itself, so no lifetime crosses an
    // unrelated await point.
    let mut frame_buf: Vec<u8> = Vec::with_capacity(4096);
    let cause: CloseCause = loop {
        match read_frame_into(&mut reader, MAX_FRAME_SIZE_DEFAULT, &mut frame_buf).await {
            Ok(()) => {}
            Err(FrameError::PeerEof) => {
                // Peer dropped the stream mid-header — same cause as a
                // graceful close from the reader's point of view.
                break CloseCause::PeerEof;
            }
            Err(FrameError::PeerClose) => {
                // Graceful close from server.
                break CloseCause::PeerEof;
            }
            Err(e) => {
                tracing::debug!("reader_task: read_frame error: {e}");
                let s: Arc<str> = Arc::from(e.to_string());
                break CloseCause::Io(s);
            }
        }
        let result = match decode_frame(&frame_buf) {
            Some(r) => r,
            None => {
                // Not a response/error envelope — try push frame.
                if let Ok(envelope) = rmp_serde::from_slice::<PushEnvelope>(&frame_buf) {
                    // Route to live sender or buffer early under the shared subscription mutex.
                    enum PushAction {
                        Live(Result<(), tokio::sync::mpsc::error::TrySendError<PushEnvelope>>),
                        Buffered,
                    }
                    // One reader task and rare subscribe calls per connection; this mutex makes route/install/flush atomic.
                    let mut subs = state.subs.lock().unwrap_or_else(|p| p.into_inner());
                    debug_assert_eq!(
                        subs.early_total,
                        subs.early.values().map(Vec::len).sum::<usize>()
                    );
                    let action = if let Some(tx) = subs.live.get(&envelope.sub) {
                        PushAction::Live(tx.try_send(envelope))
                    } else if subs.closed.is_none() {
                        if subs.early_total < EARLY_BUFFER_TOTAL_CAP {
                            let items = subs.early.entry(envelope.sub).or_default();
                            if items.len() < EARLY_BUFFER_CAP {
                                items.push(envelope);
                                subs.early_total += 1;
                            } else {
                                tracing::debug!(
                                    "reader_task: early buffer full for sub {}; push dropped",
                                    envelope.sub
                                );
                            }
                        } else {
                            tracing::debug!(
                                "reader_task: aggregate early buffer full; push for sub {} dropped",
                                envelope.sub
                            );
                        }
                        debug_assert_eq!(
                            subs.early_total,
                            subs.early.values().map(Vec::len).sum::<usize>()
                        );
                        PushAction::Buffered
                    } else {
                        PushAction::Buffered
                    };
                    if let PushAction::Live(res) = action {
                        match res {
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
            DemuxResult::Error { rid, error } => (rid, Err(ClientError::Server { code: error })),
        };

        let rid = match rid {
            Some(r) => r,
            None => {
                // Response/error envelope without rid — unusual but harmless.
                tracing::debug!("reader_task: frame without rid, dropping");
                continue;
            }
        };

        let sender = state.pending.map.remove_sync(&rid).map(|(_, tx)| tx);

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
    };

    guard.set(cause.clone());
    #[cfg(test)]
    if let Some(hook) = &hooks.reader_before_shutdown {
        hook.fire().await;
    }
    #[cfg(not(test))]
    let _ = &hooks; // zero-sized in production builds
                    // Dropping the guard runs the idempotent shutdown with `cause`.
    drop(guard);
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

/// Sent by the client as the FIRST frame when resuming with identity
/// verification (RV-CLIENT-DE75ADDD): asks the server to prove its pinned
/// Ed25519 identity before the bearer ticket is disclosed.
#[derive(Serialize)]
pub(crate) struct WireResumeProofInit {
    #[serde(with = "serde_bytes")]
    pub client_nonce: Vec<u8>,
}

/// Server answer to [`WireResumeProofInit`]: current identity pub key plus
/// an Ed25519 signature over `RESUME_PROOF_V1 || client_nonce || exporter`.
#[derive(Deserialize)]
pub(crate) struct WireResumeProofOk {
    #[serde(with = "serde_bytes")]
    pub server_pub_key: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub signature: Vec<u8>,
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
