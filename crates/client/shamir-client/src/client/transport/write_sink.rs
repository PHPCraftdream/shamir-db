use std::pin::Pin;
use std::task::{Context, Poll};

use hyper::upgrade::Upgraded;
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncWrite, WriteHalf};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;

use shamir_transport_ipc::IpcClientStream;

use super::ws_stream::WsByteStream;

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
    Tcp(WriteHalf<TcpStream>),
    Ws(WriteHalf<WsByteStream<TlsStream<TcpStream>>>),
    TokenWs(WriteHalf<WsByteStream<TokioIo<Upgraded>>>),
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
            WriteSink::Tcp(w) => Pin::new(w).poll_write(cx, buf),
            WriteSink::Ws(w) => Pin::new(w).poll_write(cx, buf),
            WriteSink::TokenWs(w) => Pin::new(w).poll_write(cx, buf),
            #[cfg(test)]
            WriteSink::Test(w) => Pin::new(&mut **w).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            WriteSink::Tls(w) => Pin::new(w).poll_flush(cx),
            WriteSink::Ipc(w) => Pin::new(w).poll_flush(cx),
            WriteSink::Tcp(w) => Pin::new(w).poll_flush(cx),
            WriteSink::Ws(w) => Pin::new(w).poll_flush(cx),
            WriteSink::TokenWs(w) => Pin::new(w).poll_flush(cx),
            #[cfg(test)]
            WriteSink::Test(w) => Pin::new(&mut **w).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            WriteSink::Tls(w) => Pin::new(w).poll_shutdown(cx),
            WriteSink::Ipc(w) => Pin::new(w).poll_shutdown(cx),
            WriteSink::Tcp(w) => Pin::new(w).poll_shutdown(cx),
            WriteSink::Ws(w) => Pin::new(w).poll_shutdown(cx),
            WriteSink::TokenWs(w) => Pin::new(w).poll_shutdown(cx),
            #[cfg(test)]
            WriteSink::Test(w) => Pin::new(&mut **w).poll_shutdown(cx),
        }
    }
}
