//! Transport plumbing for the client: the transport-agnostic write sink and
//! the bounded connect helpers.

// Re-exports: the mux/lifecycle/reader code moved to `client/session/mux/`
// but `crate::client::transport::{...}` paths must keep resolving for the
// subscription registry and the guard tests.
#[cfg(test)]
pub(crate) use crate::client::handshake::wire::{
    WireAuthInit, WireAuthOk, WireChallenge, WireClientProof, WireResumeInit, WireResumeOk,
    WireResumeProofOk,
};
#[cfg(test)]
pub(crate) use crate::client::session::mux::hooks::HookPoint;
pub(crate) use crate::client::session::mux::lifecycle::{
    ClosedState, Lifecycle, LifecycleHooks, ShutdownGuard,
};
pub(crate) use crate::client::session::mux::reader::{await_pending_response, reader_task};

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncWrite, WriteHalf};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;

use shamir_transport_ipc::IpcClientStream;

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
    connect_with_timeout(TcpStream::connect(addr), timeout).await
}

/// Establish the underlying local-IPC connection (Unix domain socket /
/// Windows Named Pipe), optionally bounded by a connect deadline. Mirrors
/// [`connect_tcp`] — see its docs for the `timeout` semantics.
pub(crate) async fn connect_ipc(
    addr: &str,
    timeout: Option<std::time::Duration>,
) -> Result<IpcClientStream, ClientError> {
    connect_with_timeout(shamir_transport_ipc::connect(addr), timeout).await
}

pub(crate) async fn connect_with_timeout<T, E>(
    connector: impl Future<Output = Result<T, E>>,
    timeout: Option<std::time::Duration>,
) -> Result<T, ClientError>
where
    ClientError: From<E>,
{
    match timeout {
        None => Ok(connector.await?),
        Some(d) => match tokio::time::timeout(d, connector).await {
            Ok(res) => Ok(res?),
            Err(_) => Err(ClientError::ConnectTimeout(d)),
        },
    }
}
