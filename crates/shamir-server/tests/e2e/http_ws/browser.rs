use super::super::http_rest::common::Harness;
use super::common::{raw_response, upgrade};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::net::TcpStream;
use tokio::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::WebSocketStream;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    ok: bool,
    #[serde(with = "serde_bytes")]
    sid: Vec<u8>,
    expires_at_ns: u64,
}

async fn browser_socket(addr: std::net::SocketAddr) -> WebSocketStream<TcpStream> {
    let mut request = format!("ws://{addr}/shamir/v1/ws/browser")
        .into_client_request()
        .expect("request URI");
    request
        .headers_mut()
        .insert("Origin", HeaderValue::from_static("https://app.example"));
    request
        .headers_mut()
        .insert("Host", HeaderValue::from_static("localhost"));
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static("shamir-v1"),
    );
    let tcp = tokio::time::timeout(Duration::from_secs(15), TcpStream::connect(addr))
        .await
        .expect("connect timeout")
        .expect("connect");
    tokio::time::timeout(Duration::from_secs(15), async {
        tokio_tungstenite::client_async(request, tcp)
            .await
            .map(|(socket, _)| socket)
    })
    .await
    .expect("browser handshake timeout")
    .expect("browser handshake")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_ws_browser_group() {
    let harness = Harness::launch_with_hook(
        "127.0.0.1:0",
        shamir_server::config::HttpListenerSettings {
            tls: Some(false),
            host_authority_allowlist: vec!["localhost".into()],
            ..Default::default()
        },
        |config| {
            config.listeners[0].browser_origin_allowlist = vec!["https://app.example".into()];
        },
    )
    .await;
    exercise_browser_group_with_harness(&harness).await;
    let report = harness.handle.shutdown().await;
    assert!(report.errors().is_empty(), "{report:?}");
}

async fn exercise_browser_group_with_harness(harness: &Harness) {
    let token = harness
        .token(
            "browser-ws",
            shamir_connect::server::token::TokenMode::ReadOnly,
        )
        .await;
    let mut socket = browser_socket(harness.addr()).await;
    #[derive(serde::Serialize)]
    struct TokenFrame<'a> {
        token: &'a str,
    }
    let token_bytes =
        rmp_serde::to_vec_named(&TokenFrame { token: &token }).expect("encode first message");
    tokio::time::timeout(
        Duration::from_secs(15),
        socket.send(tokio_tungstenite::tungstenite::Message::Binary(
            token_bytes.into(),
        )),
    )
    .await
    .expect("token write timeout")
    .expect("send first token message");
    let message = tokio::time::timeout(Duration::from_secs(15), socket.next())
        .await
        .expect("binding timeout")
        .expect("binding frame")
        .expect("binding message");
    let tokio_tungstenite::tungstenite::Message::Binary(bytes) = message else {
        panic!("binding was not binary");
    };
    let binding: Binding = rmp_serde::from_slice(&bytes).expect("binding map");
    assert!(binding.ok && binding.expires_at_ns > 0 && binding.sid.len() == 32);
    assert!(binding.sid.iter().any(|byte| *byte != 0));
    browser_refusal_matrix(harness).await;
}

async fn browser_refusal_matrix(harness: &Harness) {
    let cases = [
        ("GET /shamir/v1/ws/browser HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Protocol: shamir-v1\r\n\r\n", "400"),
        ("GET /shamir/v1/ws/browser HTTP/1.1\r\nHost: localhost\r\nOrigin: null\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Protocol: shamir-v1\r\n\r\n", "400"),
        ("GET /shamir/v1/ws/browser HTTP/1.1\r\nHost: localhost\r\nOrigin: https://evil.example\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Protocol: shamir-v1\r\n\r\n", "403"),
        ("GET /shamir/v1/ws/browser HTTP/1.1\r\nHost: localhost\r\nHost: localhost\r\nOrigin: https://app.example\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Protocol: shamir-v1\r\n\r\n", "400"),
    ];
    for (request, status) in cases {
        let (headers, _) = raw_response(harness.addr(), request).await;
        assert_eq!(headers.split_whitespace().nth(1), Some(status), "{headers}");
    }
    let malformed_origin = upgrade(
        "/shamir/v1/ws/browser",
        "localhost",
        Some("https://app.example/path"),
        "",
    );
    let (headers, _) = raw_response(harness.addr(), &malformed_origin).await;
    assert_eq!(headers.split_whitespace().nth(1), Some("400"), "{headers}");
}
