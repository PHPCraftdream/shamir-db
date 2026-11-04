//! Transport plumbing for the client: the transport-agnostic write sink and
//! the bounded connect helpers.

// Re-exports: the mux/lifecycle/reader code moved to `client/session/mux/`
// but `crate::client::transport::{...}` paths must keep resolving for the
// subscription registry and the guard tests.
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

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncWrite, WriteHalf};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;

use shamir_transport_ipc::IpcClientStream;

use crate::client::session::entry::resume_security::ProtectedResumeBuffer;
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
pub(crate) struct WireResumeInit<'a> {
    #[serde(with = "serde_bytes")]
    pub ticket: &'a [u8],
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
    #[serde(
        default,
        deserialize_with = "crate::client::session::entry::resume_security::deserialize_resume_ticket"
    )]
    pub resumption_ticket: ProtectedResumeBuffer,
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
    /// Presently informational; identity recovery requires explicit policy.
    #[serde(default)]
    pub rotation_in_progress: Option<shamir_connect::common::wire::RotationInProgressWire>,
    /// Informational KDF-upgrade request from the authenticated server.
    #[serde(default)]
    pub kdf_upgrade_required: Option<bool>,
}
