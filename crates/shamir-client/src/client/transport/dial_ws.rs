use std::net::{Ipv6Addr, SocketAddr};
use std::sync::atomic::AtomicU8;

use rustls::pki_types::ServerName;
use tokio::io::{split, ReadHalf};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;
use tokio_tungstenite::client_async_with_config;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Error as WsError;

use shamir_transport_tcp::framing::MAX_FRAME_SIZE_DEFAULT;
use shamir_transport_tcp::tls::{
    extract_tls_exporter, make_client_config_app_authenticated, AppLayerServerAuth,
};

use super::binding::ChannelBinding;
use super::dial::{connect_tcp, mark_phase, Dialed};
use super::token::{build_connector, parse_server_name, TokenTrust};
use super::write_sink::WriteSink;
use super::ws_stream::WsByteStream;
use crate::client::session::entry::{PHASE_TCP, PHASE_TLS};
use crate::error::ClientError;

const WS_PATH: &str = "/shamir/v1";
const WS_BROWSER_PATH: &str = "/shamir/v1/browser";
const WS_SUBPROTOCOL: &str = "shamir-v1";

fn upgrade_error(e: WsError) -> ClientError {
    ClientError::handshake_with("websocket upgrade", e)
}

fn upgrade_request(
    server_name: &str,
    port: u16,
    path: &str,
    origin: Option<&str>,
) -> Result<Request, ClientError> {
    let host = if server_name.parse::<Ipv6Addr>().is_ok() {
        format!("[{server_name}]")
    } else {
        server_name.to_owned()
    };
    let mut request = format!("wss://{host}:{port}{path}")
        .into_client_request()
        .map_err(upgrade_error)?;
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static(WS_SUBPROTOCOL),
    );
    // An empty origin string means the header is omitted entirely (the
    // listener answers 400 "origin required"); any non-empty value is sent
    // verbatim for the allowlist check.
    if let Some(origin) = origin.filter(|o| !o.is_empty()) {
        let value =
            HeaderValue::from_str(origin).map_err(|_| ClientError::handshake("invalid Origin"))?;
        request.headers_mut().insert("Origin", value);
    }
    Ok(request)
}

/// TCP connect → TLS (exporter-bound app-layer server auth) → TLS exporter
/// → WebSocket upgrade (`GET /shamir/v1`, subprotocol `shamir-v1`, no
/// `Origin`) → byte-stream adapter → split. The exporter is extracted from
/// the TLS stream BEFORE the upgrade, with the same label and value as on
/// TCP. Phase stores and `connect_timeout` behave as in `dial_tcp_tls`; the
/// upgrade runs under the `PHASE_TLS` phase.
pub(crate) async fn dial_ws_native(
    addr: SocketAddr,
    server_name: &str,
    connect_timeout: Option<std::time::Duration>,
    phase: Option<&AtomicU8>,
) -> Result<Dialed<ReadHalf<WsByteStream<TlsStream<TcpStream>>>>, ClientError> {
    mark_phase(phase, PHASE_TCP);
    let client_cfg = make_client_config_app_authenticated(AppLayerServerAuth::exporter_bound())
        .map_err(|e| ClientError::tls("client config", e))?;
    let connector = TlsConnector::from(client_cfg);
    let tls_name = ServerName::try_from(server_name.to_owned())
        .map_err(|e| ClientError::tls("server name", e))?;
    let tcp = connect_tcp(addr, connect_timeout).await?;
    mark_phase(phase, PHASE_TLS);
    let tls = connector.connect(tls_name, tcp).await?;
    let exporter =
        extract_tls_exporter(&tls).map_err(|e| ClientError::tls("TLS exporter unavailable", e))?;

    let request = upgrade_request(server_name, addr.port(), WS_PATH, None)?;
    let wire_ceiling = MAX_FRAME_SIZE_DEFAULT + 4;
    let config = WebSocketConfig::default()
        .max_message_size(Some(wire_ceiling))
        .max_frame_size(Some(wire_ceiling));
    let (ws, _response) = client_async_with_config(request, tls, Some(config))
        .await
        .map_err(upgrade_error)?;

    let (read, write) = split(WsByteStream::new(ws));
    Ok(Dialed {
        read,
        write: WriteSink::Ws(write),
        binding: ChannelBinding::ws_tls_exporter(exporter),
    })
}

/// TCP connect → TLS (ordinary certificate check against `trust` — the same
/// WebPKI/PEM trust plumbing as the token transports) → WebSocket upgrade
/// (`GET /shamir/v1/browser`, subprotocol `shamir-v1`, `Origin` header) →
/// byte-stream adapter → split. The browser profile has no TLS exporter: the
/// channel binding is (WebSocket, TlsNoExport, zeros), and the exporter is
/// never extracted. No ALPN is required of this listener. Phases and
/// `connect_timeout` behave as in `dial_ws_native`; the upgrade runs under
/// the `PHASE_TLS` phase.
pub(crate) async fn dial_ws_browser(
    addr: SocketAddr,
    server_name: &str,
    origin: &str,
    trust: &TokenTrust,
    connect_timeout: Option<std::time::Duration>,
    phase: Option<&AtomicU8>,
) -> Result<Dialed<ReadHalf<WsByteStream<TlsStream<TcpStream>>>>, ClientError> {
    mark_phase(phase, PHASE_TCP);
    let connector = build_connector(trust)?;
    let tls_name = parse_server_name(server_name)?;
    let tcp = connect_tcp(addr, connect_timeout).await?;
    mark_phase(phase, PHASE_TLS);
    let tls = connector
        .connect(tls_name, tcp)
        .await
        .map_err(|e| ClientError::tls("browser ws connect", e))?;

    let request = upgrade_request(server_name, addr.port(), WS_BROWSER_PATH, Some(origin))?;
    let wire_ceiling = MAX_FRAME_SIZE_DEFAULT + 4;
    let config = WebSocketConfig::default()
        .max_message_size(Some(wire_ceiling))
        .max_frame_size(Some(wire_ceiling));
    let (ws, _response) = client_async_with_config(request, tls, Some(config))
        .await
        .map_err(upgrade_error)?;

    let (read, write) = split(WsByteStream::new(ws));
    Ok(Dialed {
        read,
        write: WriteSink::Ws(write),
        binding: ChannelBinding::ws_browser(),
    })
}
