use super::super::http_rest::common::{grant_default_main_access, Harness, ADMIN_PASSWORD};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use shamir_connect::common::envelope::{RequestEnvelope, ResponseEnvelope};
use shamir_query_builder::{batch::Batch, ddl};
use shamir_query_types::wire::{DbRequest, DbResponse, CURRENT_QUERY_LANG_VERSION};
use tokio::net::TcpStream;
use tokio::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::WebSocketStream;
use zeroize::Zeroizing;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    ok: bool,
    #[serde(with = "serde_bytes")]
    sid: Vec<u8>,
    expires_at_ns: u64,
}

pub(super) async fn open_native(
    addr: std::net::SocketAddr,
    token: &str,
) -> (WebSocketStream<TcpStream>, [u8; 32]) {
    let url = format!("ws://{addr}/shamir/v1/ws");
    let mut request = url.into_client_request().expect("build WS request");
    request.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).expect("authorization header"),
    );
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static("shamir-v1"),
    );
    let stream = tokio::time::timeout(Duration::from_secs(15), TcpStream::connect(addr))
        .await
        .expect("connect timeout")
        .expect("connect");
    let (mut ws, response) = tokio::time::timeout(
        Duration::from_secs(15),
        tokio_tungstenite::client_async(request, stream),
    )
    .await
    .expect("WS handshake timeout")
    .expect("WS handshake");
    assert_eq!(response.status().as_u16(), 101);
    assert_eq!(
        response
            .headers()
            .get("sec-websocket-protocol")
            .and_then(|value| value.to_str().ok()),
        Some("shamir-v1")
    );
    assert!(!response.headers().contains_key("sec-websocket-extensions"));
    let message = tokio::time::timeout(Duration::from_secs(15), ws.next())
        .await
        .expect("binding timeout")
        .expect("binding stream")
        .expect("binding frame");
    let tokio_tungstenite::tungstenite::Message::Binary(bytes) = message else {
        panic!("binding is not binary");
    };
    let binding: Binding = rmp_serde::from_slice(&bytes).expect("decode binding");
    assert!(binding.ok && binding.expires_at_ns > 0);
    assert_eq!(binding.sid.len(), 32);
    assert!(binding.sid.iter().any(|byte| *byte != 0));
    (ws, binding.sid.as_slice().try_into().expect("32-byte sid"))
}

async fn send_request(
    ws: &mut WebSocketStream<TcpStream>,
    sid: &[u8; 32],
    rid: u32,
    request: &DbRequest,
) -> Vec<u8> {
    let body = rmp_serde::to_vec_named(request).expect("encode DB request");
    let envelope = RequestEnvelope::new(*sid, Some(rid), body)
        .to_msgpack()
        .expect("encode envelope");
    tokio::time::timeout(
        Duration::from_secs(15),
        ws.send(tokio_tungstenite::tungstenite::Message::Binary(
            envelope.into(),
        )),
    )
    .await
    .expect("request write timeout")
    .expect("send request");
    let message = tokio::time::timeout(Duration::from_secs(15), ws.next())
        .await
        .expect("reply timeout")
        .expect("reply stream")
        .expect("reply frame");
    let tokio_tungstenite::tungstenite::Message::Binary(bytes) = message else {
        panic!("reply is not binary");
    };
    bytes.to_vec()
}

pub(super) async fn exercise_native_session(harness: &Harness) {
    let admin = shamir_client::Client::connect(shamir_client::ConnectOptions {
        addr: harness.native_addr(),
        server_name: "localhost".into(),
        username: "admin".into(),
        password: Zeroizing::new(ADMIN_PASSWORD.to_vec()),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: None,
        handshake_timeout: None,
        request_timeout: None,
    })
    .await
    .expect("native SCRAM session");
    let mut create = Batch::new();
    create.create_table("items", ddl::create_table("items"));
    admin
        .execute("default", create.build())
        .await
        .expect("create table");
    grant_default_main_access(&admin).await;
    drop(admin);
    let token = harness
        .token(
            "ws-readonly",
            shamir_connect::server::token::TokenMode::ReadOnly,
        )
        .await;
    let (mut ws, sid) = open_native(harness.addr(), &token).await;
    let ping = send_request(&mut ws, &sid, 51, &DbRequest::Ping).await;
    let pong = ResponseEnvelope::from_msgpack(&ping).expect("response envelope");
    assert_eq!(pong.request_id, Some(51));
    assert!(matches!(
        rmp_serde::from_slice::<DbResponse>(&pong.res).expect("pong"),
        DbResponse::Pong
    ));
    let write = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: {
            let mut batch = Batch::new();
            batch.insert(
                "i",
                shamir_query_builder::write::insert("items")
                    .rows([shamir_query_builder::doc! { "n" => 1 }]),
            );
            batch.build()
        },
    };
    let denial = send_request(&mut ws, &sid, 52, &write).await;
    let denial = ResponseEnvelope::from_msgpack(&denial).expect("denial response envelope");
    let error = rmp_serde::from_slice::<DbResponse>(&denial.res).expect("scope response");
    assert!(
        matches!(error, DbResponse::Error { ref code, ref message } if code == "token_read_only" && message == "token_read_only")
    );
    let report_close = ws.close(None).await;
    assert!(report_close.is_ok());
}
