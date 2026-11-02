use super::super::http_rest::common::Harness;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::WebSocketStream;

async fn open_socket(addr: std::net::SocketAddr, token: &str) -> WebSocketStream<TcpStream> {
    let mut request = format!("ws://{addr}/shamir/v1/ws")
        .into_client_request()
        .expect("request URI");
    request.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).expect("authorization"),
    );
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static("shamir-v1"),
    );
    let tcp = tokio::time::timeout(Duration::from_secs(15), TcpStream::connect(addr))
        .await
        .expect("connect timeout")
        .expect("connect");
    let (mut socket, response) = tokio::time::timeout(
        Duration::from_secs(15),
        tokio_tungstenite::client_async(request, tcp),
    )
    .await
    .expect("handshake timeout")
    .expect("handshake");
    assert_eq!(response.status().as_u16(), 101);
    let binding = tokio::time::timeout(Duration::from_secs(15), socket.next())
        .await
        .expect("binding timeout")
        .expect("binding stream")
        .expect("binding");
    assert!(matches!(
        binding,
        tokio_tungstenite::tungstenite::Message::Binary(_)
    ));
    socket
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_ws_shutdown_group() {
    let harness = Harness::launch().await;
    let token = harness
        .token(
            "ws-shutdown",
            shamir_connect::server::token::TokenMode::ReadOnly,
        )
        .await;
    let mut socket = open_socket(harness.addr(), &token).await;
    let report = tokio::time::timeout(Duration::from_secs(15), harness.handle.shutdown())
        .await
        .expect("shutdown timeout");
    assert!(report.errors().is_empty(), "{report:?}");
    let message = tokio::time::timeout(Duration::from_secs(15), socket.next())
        .await
        .expect("close timeout")
        .expect("close stream")
        .expect("close frame");
    let tokio_tungstenite::tungstenite::Message::Close(Some(frame)) = message else {
        panic!("expected WebSocket close");
    };
    assert_eq!(u16::from(frame.code), 1000);
}
