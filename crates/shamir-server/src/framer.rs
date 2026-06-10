//! Transport-agnostic frame channel — abstracts the difference between
//! length-prefix-over-TLS-TCP and length-prefix-over-WebSocket-binary so
//! [`crate::connection::handle_connection`] can drive both with a single
//! code path.
//!
//! # Why a trait
//!
//! `handle_connection` does only three things to its underlying channel:
//! `read_frame_into`, `write_frame_into`, `shutdown`. Two transports
//! satisfy that contract today:
//!
//! 1. **TCP+TLS** — `tokio_rustls::server::TlsStream<TcpStream>` split into
//!    read/write halves, framing supplied by `shamir-transport-tcp`.
//! 2. **WSS** — `tokio_tungstenite::WebSocketStream<TlsStream<TcpStream>>`,
//!    framing supplied by `shamir-transport-ws` (one BINARY message per
//!    frame, with a redundant inner length prefix for defence-in-depth).
//!
//! Both are `Send` and used only via `&mut`, which lets us monomorphise
//! `handle_connection<F: Framer>` with zero virtual-dispatch overhead.
//!
//! # Splitting for future duplex loops
//!
//! Every `Framer` exposes a [`Framer::split`] method that decomposes the
//! whole framer into a ([`FrameReader`], [`FrameWriter`]) pair.  Callers
//! that need to drive the read and write directions from separate tasks
//! (future duplex request loops, M1) can call `split` after the auth
//! handshake and hand each half to its own task.
//!
//! The `handle_connection` / `run_handshake` path continues to work on the
//! whole framer; only the request loop uses the halves.
//!
//! # Non-blocking
//!
//! Every method is `async`; the underlying TCP and WS implementations are
//! tokio-driven futures. There is no blocking I/O on the runtime worker.
//! Argon2id (the only CPU-bound step in the post-handshake hot path) is
//! gated by `Argon2Semaphore::try_acquire` and runs inside
//! `tokio::task::block_in_place` from `db_handler`, both of which keep the
//! runtime responsive to other connections.

use std::future::Future;
use thiserror::Error;
use tokio::io::{split, AsyncRead, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio_tungstenite::WebSocketStream;

use futures_util::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use tokio_tungstenite::tungstenite::Message;

use shamir_transport_tcp::framing::{
    read_frame_into as tcp_read_frame_into, write_frame_into as tcp_write_frame_into,
};
use shamir_transport_ws::framing::{
    ws_recv_into_stream, ws_send_sink, WsFrameError, MAX_WS_FRAME_SIZE,
};

/// Unified error surface — collapses the per-transport error types into
/// the shape `connection.rs` cares about.
#[derive(Debug, Error)]
pub enum FramerError {
    /// Underlying I/O error (or transport-specific wire error).
    #[error("framer io: {0}")]
    Io(String),
    /// Peer closed the channel cleanly.
    #[error("peer closed")]
    PeerClose,
    /// Frame larger than the configured cap.
    #[error("frame too large: {actual} > {max}")]
    TooLarge { actual: usize, max: usize },
    /// Malformed frame at the framing layer (e.g. WS length-prefix
    /// mismatch, unexpected message type).
    #[error("framer decode: {0}")]
    Decode(String),
}

// ---------------------------------------------------------------------------
// FrameReader / FrameWriter — directional half-traits
// ---------------------------------------------------------------------------

/// Read half of a split framer.  Owns the read direction only; can be moved
/// to a dedicated reader task for future duplex request loops.
pub trait FrameReader: Send {
    /// Read one frame into `buf`. Identical semantics to
    /// [`Framer::read_frame_into`].
    fn read_frame_into<'a>(
        &'a mut self,
        max: usize,
        buf: &'a mut Vec<u8>,
    ) -> impl Future<Output = Result<(), FramerError>> + Send + 'a;
}

/// Write half of a split framer.  Owns the write direction and the shutdown
/// operation; can be moved to a dedicated writer task for future duplex
/// request loops.
pub trait FrameWriter: Send {
    /// Write one frame. Identical semantics to [`Framer::write_frame_into`].
    fn write_frame_into<'a>(
        &'a mut self,
        payload: &'a [u8],
        scratch: &'a mut Vec<u8>,
    ) -> impl Future<Output = Result<(), FramerError>> + Send + 'a;

    /// Half-close the write side; best-effort, no error reporting.
    fn shutdown<'a>(&'a mut self) -> impl Future<Output = ()> + Send + 'a;
}

// ---------------------------------------------------------------------------
// Framer — bidirectional (whole-framer) trait
// ---------------------------------------------------------------------------

/// Bidirectional frame channel — what `handle_connection` actually needs.
///
/// `read_frame_into` consumes one frame from the wire and overwrites
/// `buf`'s contents (zero-allocation in steady state because the caller
/// reuses the same `Vec`).
///
/// `write_frame_into` sends one frame; for transports that don't need a
/// scratch buffer (WS), the parameter is ignored.
///
/// `shutdown` half-closes the write side. Read side will see EOF on the
/// peer's next attempt.
///
/// `split` decomposes the framer into a ([`FrameReader`], [`FrameWriter`])
/// pair for use in duplex task layouts.  See [module-level docs](self) for
/// context.
pub trait Framer: Send {
    /// The read half produced by [`split`](Framer::split).
    type Reader: FrameReader + 'static;
    /// The write half produced by [`split`](Framer::split).
    type Writer: FrameWriter + 'static;

    /// Read one frame into `buf`. Errors are best-effort classified into
    /// [`FramerError`] variants.
    fn read_frame_into<'a>(
        &'a mut self,
        max: usize,
        buf: &'a mut Vec<u8>,
    ) -> impl Future<Output = Result<(), FramerError>> + Send + 'a;

    /// Write one frame. `scratch` is reused for zero-allocation framing
    /// where applicable (TCP); ignored for transports that build their
    /// own buffer (WS).
    fn write_frame_into<'a>(
        &'a mut self,
        payload: &'a [u8],
        scratch: &'a mut Vec<u8>,
    ) -> impl Future<Output = Result<(), FramerError>> + Send + 'a;

    /// Half-close the write side; best-effort, no error reporting.
    fn shutdown<'a>(&'a mut self) -> impl Future<Output = ()> + Send + 'a;

    /// Decompose into directional halves.  After calling this, the whole
    /// framer is consumed; use [`FrameReader`] and [`FrameWriter`] directly.
    fn split(self) -> (Self::Reader, Self::Writer);
}

// --------------------------------------------------------------------------
// TCP + TLS impl
// --------------------------------------------------------------------------

/// Read half of a TCP framer.
pub struct TcpFrameReader<S>(ReadHalf<S>);

/// Write half of a TCP framer.
pub struct TcpFrameWriter<S>(WriteHalf<S>);

impl<S> FrameReader for TcpFrameReader<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    async fn read_frame_into(&mut self, max: usize, buf: &mut Vec<u8>) -> Result<(), FramerError> {
        tcp_read_frame_into(&mut self.0, max, buf)
            .await
            .map_err(map_tcp_err)
    }
}

impl<S> FrameWriter for TcpFrameWriter<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    async fn write_frame_into(
        &mut self,
        payload: &[u8],
        scratch: &mut Vec<u8>,
    ) -> Result<(), FramerError> {
        tcp_write_frame_into(&mut self.0, payload, scratch)
            .await
            .map_err(map_tcp_err)
    }

    async fn shutdown(&mut self) {
        let _ = self.0.shutdown().await;
    }
}

/// Framer over a TLS-wrapped TCP stream. Uses `shamir-transport-tcp`'s
/// length-prefix framing.
pub struct TcpFramer<S> {
    r: ReadHalf<S>,
    w: WriteHalf<S>,
}

impl<S> TcpFramer<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    /// Split the underlying stream into read/write halves so the framer
    /// can drive read-then-write request loops without holding a single
    /// `&mut` borrow across both directions.
    pub fn new(stream: S) -> Self {
        let (r, w) = split(stream);
        Self { r, w }
    }
}

impl<S> Framer for TcpFramer<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    type Reader = TcpFrameReader<S>;
    type Writer = TcpFrameWriter<S>;

    async fn read_frame_into(&mut self, max: usize, buf: &mut Vec<u8>) -> Result<(), FramerError> {
        tcp_read_frame_into(&mut self.r, max, buf)
            .await
            .map_err(map_tcp_err)
    }

    async fn write_frame_into(
        &mut self,
        payload: &[u8],
        scratch: &mut Vec<u8>,
    ) -> Result<(), FramerError> {
        tcp_write_frame_into(&mut self.w, payload, scratch)
            .await
            .map_err(map_tcp_err)
    }

    async fn shutdown(&mut self) {
        let _ = self.w.shutdown().await;
    }

    fn split(self) -> (TcpFrameReader<S>, TcpFrameWriter<S>) {
        (TcpFrameReader(self.r), TcpFrameWriter(self.w))
    }
}

fn map_tcp_err(e: shamir_transport_tcp::framing::FrameError) -> FramerError {
    use shamir_transport_tcp::framing::FrameError;
    match e {
        FrameError::PeerClose => FramerError::PeerClose,
        FrameError::TooLarge { actual, max } => FramerError::TooLarge { actual, max },
        other => FramerError::Io(other.to_string()),
    }
}

// --------------------------------------------------------------------------
// WS impl
// --------------------------------------------------------------------------

/// Read half of a WS framer — wraps `SplitStream<WebSocketStream<S>>`.
pub struct WsFrameReader<S>(SplitStream<WebSocketStream<S>>);

/// Write half of a WS framer — wraps `SplitSink<WebSocketStream<S>, Message>`.
///
/// Shutdown sends a WS `Close` frame via `SinkExt::close`, which is the
/// idiomatic tungstenite way to initiate the WS closing handshake and flush
/// the sink before dropping it.
pub struct WsFrameWriter<S>(SplitSink<WebSocketStream<S>, Message>);

impl<S> FrameReader for WsFrameReader<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    async fn read_frame_into(&mut self, max: usize, buf: &mut Vec<u8>) -> Result<(), FramerError> {
        let effective_max = max.min(MAX_WS_FRAME_SIZE);
        ws_recv_into_stream(&mut self.0, effective_max, buf)
            .await
            .map_err(map_ws_err)
    }
}

impl<S> FrameWriter for WsFrameWriter<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    async fn write_frame_into(
        &mut self,
        payload: &[u8],
        _scratch: &mut Vec<u8>,
    ) -> Result<(), FramerError> {
        ws_send_sink(&mut self.0, payload).await.map_err(map_ws_err)
    }

    async fn shutdown(&mut self) {
        // SinkExt::close sends the WS Close frame and flushes — equivalent
        // to the whole-stream `ws.close(None).await` used by WsFramer.
        let _ = self.0.close().await;
    }
}

/// Framer over a WebSocket stream (native `wss://` or browser `wss://`).
/// Uses `shamir-transport-ws`'s BINARY-message framing — one logical
/// frame per WS message, plus a redundant inner length prefix for
/// defence-in-depth.
pub struct WsFramer<S> {
    ws: WebSocketStream<S>,
}

impl<S> WsFramer<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    /// Wrap an already-upgraded `WebSocketStream`. The TLS exporter (when
    /// applicable) MUST be extracted from the underlying TLS stream
    /// BEFORE the upgrade — once the WS owns it, it's no longer
    /// accessible from outside.
    pub fn new(ws: WebSocketStream<S>) -> Self {
        Self { ws }
    }
}

impl<S> Framer for WsFramer<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    type Reader = WsFrameReader<S>;
    type Writer = WsFrameWriter<S>;

    async fn read_frame_into(&mut self, max: usize, buf: &mut Vec<u8>) -> Result<(), FramerError> {
        // WS framing has its own ceiling; clamp to the smaller of
        // caller-supplied `max` and the WS-layer max so accidental
        // mis-config can't bypass the WS limit.
        let effective_max = max.min(MAX_WS_FRAME_SIZE);
        ws_recv_into_stream(&mut self.ws, effective_max, buf)
            .await
            .map_err(map_ws_err)
    }

    async fn write_frame_into(
        &mut self,
        payload: &[u8],
        _scratch: &mut Vec<u8>,
    ) -> Result<(), FramerError> {
        // WS already builds its own send buffer (one allocation per
        // message); the scratch is redundant here.
        ws_send_sink(&mut self.ws, payload)
            .await
            .map_err(map_ws_err)
    }

    async fn shutdown(&mut self) {
        let _ = self.ws.close(None).await;
    }

    fn split(self) -> (WsFrameReader<S>, WsFrameWriter<S>) {
        let (w, r) = self.ws.split();
        (WsFrameReader(r), WsFrameWriter(w))
    }
}

fn map_ws_err(e: WsFrameError) -> FramerError {
    match e {
        WsFrameError::PeerClose => FramerError::PeerClose,
        WsFrameError::TooLarge { actual, max } => FramerError::TooLarge { actual, max },
        WsFrameError::Io(io) => FramerError::Io(io.to_string()),
        WsFrameError::LengthMismatch { declared, actual } => FramerError::Decode(format!(
            "ws length mismatch: declared={declared}, actual={actual}"
        )),
        WsFrameError::NonBinaryMessage(s) => {
            FramerError::Decode(format!("ws non-binary message: {s}"))
        }
    }
}
