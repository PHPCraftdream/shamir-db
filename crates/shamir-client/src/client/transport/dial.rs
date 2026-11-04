use std::future::Future;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU8, Ordering};

use rustls::pki_types::ServerName;
use tokio::io::{split, ReadHalf};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;

use shamir_transport_ipc::IpcClientStream;
use shamir_transport_tcp::tls::{
    extract_tls_exporter, make_client_config_app_authenticated, AppLayerServerAuth,
};

use super::binding::ChannelBinding;
use super::write_sink::WriteSink;
use crate::client::session::entry::{PHASE_TCP, PHASE_TLS};
use crate::error::ClientError;

/// The transport side of a freshly dialed connection: the read half, the
/// write sink and the channel binding the handshake must be bound to.
pub(crate) struct Dialed<R> {
    pub(in crate::client) read: R,
    pub(in crate::client) write: WriteSink,
    pub(in crate::client) binding: ChannelBinding,
}

fn mark_phase(phase: Option<&AtomicU8>, value: u8) {
    if let Some(phase) = phase {
        phase.store(value, Ordering::Relaxed);
    }
}

/// TCP connect → TLS (exporter-bound app-layer server auth) → split. The
/// phase stores (`PHASE_TCP`, then `PHASE_TLS`) feed the handshake-timeout
/// diagnostics; `connect_timeout` bounds only the TCP-connect step.
pub(crate) async fn dial_tcp_tls(
    addr: SocketAddr,
    server_name: &str,
    connect_timeout: Option<std::time::Duration>,
    phase: Option<&AtomicU8>,
) -> Result<Dialed<ReadHalf<TlsStream<TcpStream>>>, ClientError> {
    mark_phase(phase, PHASE_TCP);
    let client_cfg = make_client_config_app_authenticated(AppLayerServerAuth::exporter_bound())
        .map_err(|e| ClientError::tls("client config", e))?;
    let connector = TlsConnector::from(client_cfg);
    let server_name = ServerName::try_from(server_name.to_owned())
        .map_err(|e| ClientError::tls("server name", e))?;
    let tcp = connect_tcp(addr, connect_timeout).await?;
    mark_phase(phase, PHASE_TLS);
    let tls = connector.connect(server_name, tcp).await?;
    let exporter =
        extract_tls_exporter(&tls).map_err(|e| ClientError::tls("TLS exporter unavailable", e))?;

    let (read, write) = split(tls);
    Ok(Dialed {
        read,
        write: WriteSink::Tls(write),
        binding: ChannelBinding::tcp_tls_exporter(exporter),
    })
}

/// Local IPC connect (no TLS) → split.
pub(crate) async fn dial_ipc(
    addr: &str,
    connect_timeout: Option<std::time::Duration>,
) -> Result<Dialed<ReadHalf<IpcClientStream>>, ClientError> {
    let stream = connect_ipc(addr, connect_timeout).await?;
    let (read, write) = split(stream);
    Ok(Dialed {
        read,
        write: WriteSink::Ipc(write),
        binding: ChannelBinding::ipc(),
    })
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
