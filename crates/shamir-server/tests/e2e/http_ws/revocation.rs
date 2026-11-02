use super::super::http_rest::common::Harness;
use futures_util::{SinkExt, StreamExt};
use shamir_connect::server::token::{parse_token, TokenId};
use std::time::{SystemTime, UNIX_EPOCH};
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
    let (mut ws, response) = tokio::time::timeout(
        Duration::from_secs(15),
        tokio_tungstenite::client_async(request, tcp),
    )
    .await
    .expect("handshake timeout")
    .expect("handshake");
    assert_eq!(response.status().as_u16(), 101);
    let binding = tokio::time::timeout(Duration::from_secs(15), ws.next())
        .await
        .expect("binding timeout")
        .expect("binding frame")
        .expect("binding message");
    assert!(matches!(
        binding,
        tokio_tungstenite::tungstenite::Message::Binary(_)
    ));
    ws
}

fn token_id(token: &str) -> TokenId {
    parse_token(token.as_bytes())
        .expect("parse canonical token")
        .id
}

fn now_ns() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos(),
    )
    .expect("timestamp fits")
}

async fn expect_close(ws: &mut WebSocketStream<TcpStream>, expected: u16) {
    let message = tokio::time::timeout(Duration::from_secs(15), ws.next())
        .await
        .expect("close timeout")
        .expect("close message")
        .expect("close frame");
    let tokio_tungstenite::tungstenite::Message::Close(Some(frame)) = message else {
        panic!("expected websocket close frame");
    };
    assert_eq!(u16::from(frame.code), expected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_ws_revocation_expiry_group() {
    let harness = Harness::launch().await;
    let token = harness
        .token(
            "ws-revoke",
            shamir_connect::server::token::TokenMode::ReadOnly,
        )
        .await;
    let id = token_id(&token);
    let mut ws = open_socket(harness.addr(), &token).await;
    assert_eq!(
        harness.handle.token_service.sessions().count_for_token(&id),
        1
    );
    assert!(harness
        .handle
        .token_service
        .revoke(id, now_ns())
        .await
        .expect("revoke token"));
    expect_close(&mut ws, 4011).await;
    assert_eq!(
        harness.handle.token_service.sessions().count_for_token(&id),
        0
    );
    drop(ws);
    let expiring = harness
        .token_with_ttl(
            "ws-expire",
            shamir_connect::server::token::TokenMode::ReadOnly,
            2,
        )
        .await;
    let expiring_id = token_id(&expiring);
    let mut expiring_ws = open_socket(harness.addr(), &expiring).await;
    expect_close(&mut expiring_ws, 4012).await;
    assert_eq!(
        harness
            .handle
            .token_service
            .sessions()
            .count_for_token(&expiring_id),
        0
    );
    drop(expiring_ws);
    let user_token = harness
        .token(
            "ws-revoke-user",
            shamir_connect::server::token::TokenMode::ReadOnly,
        )
        .await;
    let user_id = token_id(&user_token);
    let mut user_ws = open_socket(harness.addr(), &user_token).await;
    assert_eq!(
        harness
            .handle
            .token_service
            .revoke_user(harness.owner, now_ns())
            .await
            .expect("revoke owner tokens"),
        1
    );
    expect_close(&mut user_ws, 4011).await;
    assert_eq!(
        harness
            .handle
            .token_service
            .sessions()
            .count_for_token(&user_id),
        0
    );
    drop(user_ws);
    let report = harness.handle.shutdown().await;
    assert!(report.errors().is_empty(), "{report:?}");
}
