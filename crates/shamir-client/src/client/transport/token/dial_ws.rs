use std::sync::atomic::AtomicU8;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Limited};
use hyper::client::conn::http1;
use hyper::header::{
    HeaderMap, HeaderValue, AUTHORIZATION, CONNECTION, HOST, RETRY_AFTER, SEC_WEBSOCKET_ACCEPT,
    SEC_WEBSOCKET_EXTENSIONS, SEC_WEBSOCKET_KEY, SEC_WEBSOCKET_PROTOCOL, SEC_WEBSOCKET_VERSION,
    UPGRADE,
};
use hyper::upgrade::Upgraded;
use hyper::{Request, StatusCode, Uri};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};
use tokio_tungstenite::WebSocketStream;

use shamir_transport_tcp::framing::MAX_FRAME_SIZE_DEFAULT;

use super::http_conn::TokenReply;
use super::status::{map_upgrade_reply, parse_retry_after};
use super::{build_connector, parse_server_name, TokenEndpoint, TokenScheme};
use crate::client::session::entry::{PHASE_TCP, PHASE_TLS};
use crate::client::transport::dial::{connect_tcp, mark_phase};
use crate::error::ClientError;

const PATH_WS: &str = "/shamir/v1/ws";
const WS_SUBPROTOCOL: &str = "shamir-v1";
const MAX_BUF_SIZE: usize = 64 * 1024;
const MAX_REJECT_BODY: usize = 64 * 1024;

/// TCP connect → (TLS, ALPN `http/1.1`) → `GET /shamir/v1/ws` upgrade with
/// `Authorization` and subprotocol `shamir-v1` (no `Origin`). `connect_timeout`
/// bounds only the TCP connect; TLS and the upgrade are bounded by the
/// caller's handshake timeout. A non-101 answer is mapped like a REST failure.
pub(crate) async fn dial_token_ws(
    endpoint: &TokenEndpoint,
    authorization: HeaderValue,
    connect_timeout: Option<Duration>,
    phase: Option<&AtomicU8>,
) -> Result<WebSocketStream<TokioIo<Upgraded>>, ClientError> {
    mark_phase(phase, PHASE_TCP);
    let tls = match endpoint.scheme() {
        TokenScheme::Https(trust) => Some((
            build_connector(trust)?,
            parse_server_name(endpoint.server_name())?,
        )),
        TokenScheme::PlainLoopback => None,
    };
    let host = HeaderValue::from_str(&endpoint.host_header())
        .map_err(|_| ClientError::handshake("invalid host header"))?;
    let tcp = connect_tcp(endpoint.addr(), connect_timeout).await?;
    let _ = tcp.set_nodelay(true);
    let Some((connector, server_name)) = tls else {
        return upgrade(tcp, host, authorization).await;
    };
    mark_phase(phase, PHASE_TLS);
    let stream = connector
        .connect(server_name, tcp)
        .await
        .map_err(|e| ClientError::tls("token ws connect", e))?;
    if stream.get_ref().1.alpn_protocol() != Some(&b"http/1.1"[..]) {
        return Err(ClientError::handshake("server did not negotiate http/1.1"));
    }
    upgrade(stream, host, authorization).await
}

async fn upgrade<I>(
    io: I,
    host: HeaderValue,
    authorization: HeaderValue,
) -> Result<WebSocketStream<TokioIo<Upgraded>>, ClientError>
where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut builder = http1::Builder::new();
    builder.max_buf_size(MAX_BUF_SIZE);
    let (mut sender, conn) = builder
        .handshake::<_, Empty<Bytes>>(TokioIo::new(io))
        .await
        .map_err(|e| ClientError::handshake_with("http1 handshake", e))?;
    tokio::spawn(async move {
        let _ = conn.with_upgrades().await;
    });

    let key = generate_key();
    let key_value = HeaderValue::from_str(&key)
        .map_err(|_| ClientError::handshake("websocket upgrade: invalid key"))?;
    let mut request = Request::new(Empty::<Bytes>::new());
    *request.uri_mut() = Uri::from_static(PATH_WS);
    let headers = request.headers_mut();
    headers.insert(HOST, host);
    headers.insert(AUTHORIZATION, authorization);
    headers.insert(CONNECTION, HeaderValue::from_static("Upgrade"));
    headers.insert(UPGRADE, HeaderValue::from_static("websocket"));
    headers.insert(SEC_WEBSOCKET_VERSION, HeaderValue::from_static("13"));
    headers.insert(SEC_WEBSOCKET_KEY, key_value);
    headers.insert(
        SEC_WEBSOCKET_PROTOCOL,
        HeaderValue::from_static(WS_SUBPROTOCOL),
    );

    sender
        .ready()
        .await
        .map_err(|e| ClientError::handshake_with("websocket upgrade", e))?;
    let response = sender
        .send_request(request)
        .await
        .map_err(|e| ClientError::handshake_with("websocket upgrade", e))?;

    if response.status() != StatusCode::SWITCHING_PROTOCOLS {
        let (parts, body) = response.into_parts();
        let body = match Limited::new(body, MAX_REJECT_BODY).collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(_) => Bytes::new(),
        };
        return Err(map_upgrade_reply(TokenReply {
            status: parts.status.as_u16(),
            content_type_msgpack: false,
            has_content_encoding: false,
            retry_after: parts
                .headers
                .get(RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(parse_retry_after),
            body,
        }));
    }

    verify_upgrade_headers(response.headers(), &key)?;

    let upgraded = hyper::upgrade::on(response)
        .await
        .map_err(|e| ClientError::handshake_with("websocket upgrade", e))?;
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_FRAME_SIZE_DEFAULT))
        .max_frame_size(Some(MAX_FRAME_SIZE_DEFAULT));
    Ok(WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Client, Some(config)).await)
}

fn verify_upgrade_headers(headers: &HeaderMap, key: &str) -> Result<(), ClientError> {
    let expected = derive_accept_key(key.as_bytes());
    let accept_ok = headers
        .get(SEC_WEBSOCKET_ACCEPT)
        .is_some_and(|v| v.as_bytes() == expected.as_bytes());
    if !accept_ok {
        return Err(upgrade_violation("bad accept"));
    }
    let mut protocols = headers.get_all(SEC_WEBSOCKET_PROTOCOL).iter();
    let protocol_ok = matches!(
        (protocols.next(), protocols.next()),
        (Some(v), None) if v.as_bytes() == WS_SUBPROTOCOL.as_bytes()
    );
    if !protocol_ok {
        return Err(upgrade_violation("bad subprotocol"));
    }
    if headers.contains_key(SEC_WEBSOCKET_EXTENSIONS) {
        return Err(upgrade_violation("unexpected extensions"));
    }
    let upgrade_ok = headers
        .get(UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("websocket"));
    if !upgrade_ok {
        return Err(upgrade_violation("bad upgrade header"));
    }
    let connection_ok = headers
        .get_all(CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|token| token.trim().eq_ignore_ascii_case("upgrade"));
    if !connection_ok {
        return Err(upgrade_violation("bad connection header"));
    }
    Ok(())
}

fn upgrade_violation(what: &str) -> ClientError {
    ClientError::handshake(format!("websocket upgrade: {what}"))
}
