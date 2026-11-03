use super::super::http_rest::common::{Harness, ADMIN_PASSWORD};
use super::session_loop::{open_native, send_request};
use futures_util::StreamExt;
use shamir_connect::common::envelope::ResponseEnvelope;
use shamir_connect::server::token::parse_token;
use shamir_query_types::wire::{DbRequest, DbResponse};
use tokio::net::TcpStream;
use tokio::time::Duration;
use tokio_tungstenite::WebSocketStream;

async fn open_socket(
    addr: std::net::SocketAddr,
    token: &str,
) -> (WebSocketStream<TcpStream>, [u8; 32]) {
    let (socket, sid, _) = open_native(addr, token).await;
    (socket, sid)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_ws_shutdown_group() {
    let harness = Harness::launch_with_native_seed_hook(
        "127.0.0.1:0",
        shamir_server::config::HttpListenerSettings {
            tls: Some(false),
            ..Default::default()
        },
        |config| {
            config.tokens.max_sockets_per_token = 16;
            config.tokens.max_sockets_per_owner = 16;
        },
    )
    .await;
    let token = harness
        .token(
            "ws-shutdown",
            shamir_connect::server::token::TokenMode::ReadOnly,
        )
        .await;
    let mut sockets = Vec::with_capacity(16);
    for _ in 0..16 {
        sockets.push(open_socket(harness.addr(), &token).await);
    }
    assert_eq!(sockets.len(), 16, "all token sockets remain open");
    let token_id = parse_token(token.as_bytes()).expect("parse token").id;
    assert_eq!(
        harness
            .handle
            .token_service
            .sessions()
            .count_for_token(&token_id),
        16,
        "the session store tracks all sixteen token sessions"
    );

    let admin = shamir_client::Client::connect(shamir_client::ConnectOptions {
        addr: harness.native_addr(),
        server_name: "localhost".into(),
        username: "admin".into(),
        password: zeroize::Zeroizing::new(ADMIN_PASSWORD.to_vec()),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: None,
        handshake_timeout: None,
        request_timeout: None,
    })
    .await
    .expect("SCRAM login despite sixteen active token sessions");
    tokio::time::timeout(Duration::from_secs(15), admin.ping())
        .await
        .expect("admin ping timeout")
        .expect("admin ping");
    assert_eq!(
        harness
            .handle
            .token_service
            .sessions()
            .count_for_token(&token_id),
        16,
        "all token sessions remain stored while admin is active"
    );
    for (rid, (socket, sid)) in sockets.iter_mut().enumerate() {
        let bytes = send_request(socket, sid, rid as u32 + 1, &DbRequest::Ping).await;
        let response = ResponseEnvelope::from_msgpack(&bytes).expect("response envelope");
        assert!(matches!(
            rmp_serde::from_slice::<DbResponse>(&response.res).expect("pong"),
            DbResponse::Pong
        ));
    }
    assert_eq!(
        harness
            .handle
            .token_service
            .sessions()
            .count_for_token(&token_id),
        16,
        "all token sessions remain stored after token pings"
    );
    drop(admin);

    let report = tokio::time::timeout(Duration::from_secs(15), harness.handle.shutdown())
        .await
        .expect("shutdown timeout");
    assert!(report.errors().is_empty(), "{report:?}");
    for (mut socket, _) in sockets {
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
}
