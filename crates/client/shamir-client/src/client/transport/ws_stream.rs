//! Byte-stream view of a WebSocket: every BINARY message carries bytes of the
//! `[u32_be length][payload]` frame stream, so the TCP framing code runs
//! unchanged over it.

use std::fmt;
use std::io::{self, ErrorKind};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{ready, Context, Poll};

use bytes::Bytes;
use futures::{Sink, Stream};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::WebSocketStream;
use zeroize::Zeroize;

use shamir_transport_tcp::framing::{FrameError, MAX_FRAME_SIZE_DEFAULT};

use crate::error::CloseCause;

const PREFIX_LEN: usize = 4;
const MAX_REASON_LEN: usize = 123;

/// How a [`WsByteStream`] maps frames onto WebSocket messages.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WsFraming {
    /// Messages carry the `[u32_be len][payload]` bytes verbatim.
    Prefixed,
    /// Messages carry only the payload; the length prefix is added on read
    /// and stripped on write.
    Raw,
}

/// The peer's WebSocket Close frame, surfaced through an `io::Error`
/// (kind `ConnectionAborted`) so the reader can recover the close code.
#[derive(Clone, Debug)]
pub(crate) struct WsPeerClose {
    pub(crate) code: u16,
    pub(crate) reason: Arc<str>,
}

impl WsPeerClose {
    pub(crate) fn new(code: u16, reason: &str) -> Self {
        let clean: String = reason
            .bytes()
            .filter(|b| (0x20..=0x7e).contains(b))
            .take(MAX_REASON_LEN)
            .map(char::from)
            .collect();
        Self {
            code,
            reason: Arc::from(clean),
        }
    }

    /// The peer's close, when `e` is the `io::Error` this stream raised for it.
    pub(crate) fn cause_of(e: &FrameError) -> Option<CloseCause> {
        match e {
            FrameError::Io(io_err) => io_err
                .get_ref()
                .and_then(|inner| inner.downcast_ref::<WsPeerClose>())
                .map(|p| p.clone().into_cause()),
            _ => None,
        }
    }

    pub(crate) fn into_cause(self) -> CloseCause {
        CloseCause::PeerClosed {
            code: self.code,
            reason: self.reason,
        }
    }
}

impl fmt::Display for WsPeerClose {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "peer closed: {} {}", self.code, self.reason)
    }
}

impl std::error::Error for WsPeerClose {}

/// Adapts a [`WebSocketStream`] to `AsyncRead + AsyncWrite`.
///
/// Read side: each received BINARY message is appended to an internal buffer
/// and served to `poll_read`; message boundaries are irrelevant because the
/// bytes already carry the frame length prefix. Ping/Pong are skipped
/// (tungstenite answers pings while the socket is polled), a Close message
/// or end of stream is EOF, TEXT is `InvalidData`.
///
/// Write side: bytes are buffered until one complete `[u32_be len][payload]`
/// frame is present and are then sent as exactly ONE BINARY message (the
/// server requires the prefix to equal the message body length minus 4). At
/// most one frame is buffered: `poll_write` accepts only the bytes that
/// belong to the current frame.
///
/// Raw mode (`WsByteStream::raw`) sends only the payload as the message and
/// serves each received message with a synthesized length prefix; a Close
/// frame carrying a code is reported once as a [`WsPeerClose`] error before
/// EOF.
pub(crate) struct WsByteStream<S> {
    ws: WebSocketStream<S>,
    framing: WsFraming,
    read_buf: Vec<u8>,
    read_pos: usize,
    read_eof: bool,
    pending_close: Option<WsPeerClose>,
    /// Close seen in Raw mode; later writes report it instead of `SendAfterClosing`.
    peer_close: Option<WsPeerClose>,
    write_buf: Vec<u8>,
}

impl<S> WsByteStream<S> {
    pub(crate) fn new(ws: WebSocketStream<S>) -> Self {
        Self::with_framing(ws, WsFraming::Prefixed)
    }

    pub(crate) fn raw(ws: WebSocketStream<S>) -> Self {
        Self::with_framing(ws, WsFraming::Raw)
    }

    fn with_framing(ws: WebSocketStream<S>, framing: WsFraming) -> Self {
        Self {
            ws,
            framing,
            read_buf: Vec::new(),
            read_pos: 0,
            read_eof: false,
            pending_close: None,
            peer_close: None,
            write_buf: Vec::new(),
        }
    }
}

impl<S> Drop for WsByteStream<S> {
    fn drop(&mut self) {
        self.write_buf.zeroize();
        self.read_buf.zeroize();
    }
}

fn ws_to_io(e: WsError) -> io::Error {
    match e {
        WsError::Io(e) => e,
        WsError::ConnectionClosed | WsError::AlreadyClosed => {
            io::Error::from(ErrorKind::BrokenPipe)
        }
        other => io::Error::new(ErrorKind::InvalidData, other),
    }
}

impl<S> WsByteStream<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    fn declared_len(&self) -> Option<usize> {
        let prefix = self.write_buf.get(..PREFIX_LEN)?;
        Some(u32::from_be_bytes([prefix[0], prefix[1], prefix[2], prefix[3]]) as usize)
    }

    fn check_declared(&self) -> io::Result<()> {
        match self.declared_len() {
            Some(len) if len > MAX_FRAME_SIZE_DEFAULT => Err(io::Error::new(
                ErrorKind::InvalidData,
                "declared frame length exceeds the frame limit",
            )),
            Some(0) if self.framing == WsFraming::Raw => Err(io::Error::new(
                ErrorKind::InvalidData,
                "declared frame length is zero",
            )),
            _ => Ok(()),
        }
    }

    fn frame_complete(&self) -> bool {
        self.declared_len()
            .is_some_and(|len| self.write_buf.len() >= PREFIX_LEN + len)
    }

    fn take_message(&mut self) -> Message {
        match self.framing {
            WsFraming::Prefixed => Message::Binary(std::mem::take(&mut self.write_buf).into()),
            WsFraming::Raw => Message::Binary(
                Bytes::from(std::mem::take(&mut self.write_buf)).slice(PREFIX_LEN..),
            ),
        }
    }

    fn poll_send_queued(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if !self.frame_complete() {
            return Poll::Ready(Ok(()));
        }
        ready!(Pin::new(&mut self.ws).poll_ready(cx)).map_err(ws_to_io)?;
        let message = self.take_message();
        Pin::new(&mut self.ws)
            .start_send(message)
            .map_err(ws_to_io)?;
        Poll::Ready(Ok(()))
    }
}

impl<S> AsyncRead for WsByteStream<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            if this.read_pos < this.read_buf.len() {
                let n = buf.remaining().min(this.read_buf.len() - this.read_pos);
                buf.put_slice(&this.read_buf[this.read_pos..this.read_pos + n]);
                this.read_pos += n;
                return Poll::Ready(Ok(()));
            }
            if let Some(close) = this.pending_close.take() {
                return Poll::Ready(Err(io::Error::new(ErrorKind::ConnectionAborted, close)));
            }
            if this.read_eof || buf.remaining() == 0 {
                return Poll::Ready(Ok(()));
            }
            match ready!(Pin::new(&mut this.ws).poll_next(cx)) {
                Some(Ok(Message::Close(Some(frame)))) if this.framing == WsFraming::Raw => {
                    this.read_eof = true;
                    let close = WsPeerClose::new(u16::from(frame.code), &frame.reason);
                    this.peer_close = Some(close.clone());
                    this.pending_close = Some(close);
                }
                None | Some(Ok(Message::Close(_))) => this.read_eof = true,
                Some(Ok(Message::Binary(bytes))) => {
                    if this.framing == WsFraming::Raw
                        && (bytes.is_empty() || bytes.len() > MAX_FRAME_SIZE_DEFAULT)
                    {
                        return Poll::Ready(Err(io::Error::new(
                            ErrorKind::InvalidData,
                            "raw WebSocket message length outside the frame limit",
                        )));
                    }
                    this.read_buf.clear();
                    if this.framing == WsFraming::Raw {
                        this.read_buf
                            .extend_from_slice(&(bytes.len() as u32).to_be_bytes());
                    }
                    this.read_buf.extend_from_slice(&bytes);
                    this.read_pos = 0;
                }
                Some(Ok(Message::Text(_))) => {
                    return Poll::Ready(Err(io::Error::new(
                        ErrorKind::InvalidData,
                        "unexpected WebSocket TEXT message",
                    )));
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
                Some(Err(WsError::ConnectionClosed | WsError::AlreadyClosed)) => {
                    this.read_eof = true;
                }
                Some(Err(e)) => return Poll::Ready(Err(ws_to_io(e))),
            }
        }
    }
}

impl<S> AsyncWrite for WsByteStream<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if let Some(close) = &this.peer_close {
            return Poll::Ready(Err(io::Error::new(
                ErrorKind::ConnectionAborted,
                close.clone(),
            )));
        }
        this.check_declared()?;
        ready!(this.poll_send_queued(cx))?;
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let want = match this.declared_len() {
            None => PREFIX_LEN - this.write_buf.len(),
            Some(len) => PREFIX_LEN + len - this.write_buf.len(),
        };
        let take = data.len().min(want);
        this.write_buf.extend_from_slice(&data[..take]);
        this.check_declared()?;
        if let Poll::Ready(Err(e)) = this.poll_send_queued(cx) {
            return Poll::Ready(Err(e));
        }
        Poll::Ready(Ok(take))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_send_queued(cx))?;
        Pin::new(&mut this.ws).poll_flush(cx).map_err(ws_to_io)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_send_queued(cx))?;
        match ready!(Pin::new(&mut this.ws).poll_close(cx)) {
            Ok(()) | Err(WsError::ConnectionClosed | WsError::AlreadyClosed) => Poll::Ready(Ok(())),
            Err(e) => Poll::Ready(Err(ws_to_io(e))),
        }
    }
}
