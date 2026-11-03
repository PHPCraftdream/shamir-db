use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::http_rest::common::{ping_body, Harness, ADMIN_PASSWORD};
use crate::token_matrix::support::DEADLINE;
use futures_util::{SinkExt, StreamExt};
use shamir_client::{Client, ConnectOptions};
use shamir_connect::common::envelope::{RequestEnvelope, ResponseEnvelope};
use shamir_connect::server::token::{parse_token, TokenMode};
use shamir_query_types::wire::{DbRequest, DbResponse};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::task::JoinSet;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;
use zeroize::Zeroizing;

const WORKERS: usize = 2;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    ok: bool,
    #[serde(with = "serde_bytes")]
    sid: Vec<u8>,
    expires_at_ns: u64,
}

fn token_id(token: &str) -> shamir_connect::server::token::TokenId {
    parse_token(token.as_bytes())
        .expect("parse canonical token")
        .id
}

async fn connect_admin(harness: &Harness) -> Client {
    tokio::time::timeout(
        DEADLINE,
        Client::connect(ConnectOptions {
            addr: harness.native_addr(),
            server_name: "localhost".into(),
            username: "admin".into(),
            password: Zeroizing::new(ADMIN_PASSWORD.to_vec()),
            accept_new_host: true,
            trusted_pin: None,
            connect_timeout: Some(DEADLINE),
            handshake_timeout: Some(DEADLINE),
            request_timeout: Some(DEADLINE),
        }),
    )
    .await
    .expect("admin SCRAM connect timeout")
    .expect("admin SCRAM connect")
}

async fn open_ws(
    addr: std::net::SocketAddr,
    token: &str,
) -> (WebSocketStream<TcpStream>, [u8; 32]) {
    let mut request = format!("ws://{addr}/shamir/v1/ws")
        .into_client_request()
        .expect("websocket request");
    request.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).expect("authorization header"),
    );
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static("shamir-v1"),
    );
    let tcp = tokio::time::timeout(DEADLINE, TcpStream::connect(addr))
        .await
        .expect("websocket connect timeout")
        .expect("websocket connect");
    let (mut socket, response) =
        tokio::time::timeout(DEADLINE, tokio_tungstenite::client_async(request, tcp))
            .await
            .expect("websocket handshake timeout")
            .expect("websocket handshake");
    assert_eq!(response.status().as_u16(), 101);
    let message = tokio::time::timeout(DEADLINE, socket.next())
        .await
        .expect("binding timeout")
        .expect("binding stream")
        .expect("binding message");
    let Message::Binary(bytes) = message else {
        panic!("expected binding binary frame")
    };
    let binding: Binding = rmp_serde::from_slice(&bytes).expect("decode binding");
    assert!(binding.ok && binding.expires_at_ns > 0 && binding.sid.len() == 32);
    let sid = binding
        .sid
        .as_slice()
        .try_into()
        .expect("32-byte session id");
    (socket, sid)
}

async fn send_ws_ping(
    socket: &mut WebSocketStream<TcpStream>,
    sid: [u8; 32],
    rid: u32,
) -> Result<(), tokio_tungstenite::tungstenite::Error> {
    let body = rmp_serde::to_vec_named(&DbRequest::Ping).expect("encode ping");
    let envelope = RequestEnvelope::new(sid, Some(rid), body)
        .to_msgpack()
        .expect("encode request envelope");
    tokio::time::timeout(DEADLINE, socket.send(Message::Binary(envelope.into())))
        .await
        .expect("websocket send timeout")
}

async fn expect_ws_pong(socket: &mut WebSocketStream<TcpStream>, rid: u32) {
    let message = tokio::time::timeout(DEADLINE, socket.next())
        .await
        .expect("websocket ping response timeout")
        .expect("websocket response stream")
        .expect("websocket response");
    let Message::Binary(bytes) = message else {
        panic!("expected websocket ping response, got {message:?}")
    };
    let response = ResponseEnvelope::from_msgpack(&bytes).expect("decode response envelope");
    assert_eq!(response.request_id, Some(rid));
    assert!(matches!(
        rmp_serde::from_slice::<DbResponse>(&response.res),
        Ok(DbResponse::Pong)
    ));
}

async fn read_head(stream: &mut TcpStream, deadline: tokio::time::Instant) -> String {
    let mut bytes = Vec::new();
    loop {
        assert!(bytes.len() < 8192, "HTTP response head too large");
        let mut byte = [0; 1];
        let n = tokio::time::timeout_at(deadline, stream.read(&mut byte))
            .await
            .expect("HTTP response head timeout")
            .expect("HTTP response head read");
        assert_ne!(n, 0, "connection ended before HTTP response head");
        bytes.push(byte[0]);
        if bytes.ends_with(b"\r\n\r\n") {
            return String::from_utf8(bytes).expect("HTTP response head UTF-8");
        }
    }
}

async fn read_http_response(
    stream: &mut TcpStream,
    deadline: tokio::time::Instant,
    initial_head: String,
) -> (u16, Vec<u8>) {
    let length = crate::token_matrix::support::content_length(&initial_head).unwrap_or(0);
    assert!(length <= 1024 * 1024, "HTTP response body exceeds test cap");
    let mut body = vec![0; length];
    if length > 0 {
        tokio::time::timeout_at(deadline, stream.read_exact(&mut body))
            .await
            .expect("HTTP response body timeout")
            .expect("HTTP response body read");
    }
    let status = initial_head
        .split_whitespace()
        .nth(1)
        .expect("HTTP status")
        .parse()
        .expect("numeric HTTP status");
    (status, body)
}

async fn one_rest_request(
    addr: std::net::SocketAddr,
    token: &str,
    revoked: &AtomicBool,
    hold_partial: bool,
    ready: Option<Arc<tokio::sync::Barrier>>,
    release: Option<tokio::sync::oneshot::Receiver<()>>,
) -> (bool, u16) {
    let after_flag = revoked.load(Ordering::Acquire);
    let mut stream = tokio::time::timeout(DEADLINE, TcpStream::connect(addr))
        .await
        .expect("REST connect timeout")
        .expect("REST connect");
    let body = ping_body();
    let head = format!(
        "POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nContent-Type: application/msgpack\r\nContent-Length: {}\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n",
        body.len()
    );
    tokio::time::timeout(DEADLINE, stream.write_all(head.as_bytes()))
        .await
        .expect("REST request head timeout")
        .expect("write REST request head");
    let deadline = tokio::time::Instant::now() + DEADLINE;
    let response_head = read_head(&mut stream, deadline).await;
    if response_head.starts_with("HTTP/1.1 100") {
        assert!(
            hold_partial,
            "unexpected interim response to post-revoke request"
        );
        tokio::time::timeout(DEADLINE, stream.write_all(&body[..1]))
            .await
            .expect("partial REST body write timeout")
            .expect("write partial REST body");
        let ready = ready.expect("in-flight readiness barrier");
        tokio::time::timeout(DEADLINE, ready.wait())
            .await
            .expect("in-flight readiness barrier timeout");
        let release = release.expect("partial request release signal");
        tokio::time::timeout(DEADLINE, release)
            .await
            .expect("partial request release timeout")
            .expect("partial request release signal dropped");
        tokio::time::timeout(DEADLINE, stream.write_all(&body[1..]))
            .await
            .expect("REST body completion timeout")
            .expect("complete REST body");
        let final_head = read_head(&mut stream, deadline).await;
        let (status, response) = read_http_response(&mut stream, deadline, final_head).await;
        assert_eq!(
            status, 401,
            "in-flight request must be unauthorized after revoke"
        );
        assert_eq!(response, b"\x81\xa5error\xacunauthorized");
        (after_flag, status)
    } else {
        let (status, response) = read_http_response(&mut stream, deadline, response_head).await;
        assert_eq!(status, 401, "rejected request must not proceed to body");
        assert_eq!(response, b"\x81\xa5error\xacunauthorized");
        (after_flag, status)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rest_and_websocket_revoke_race_is_deterministic() {
    let harness = Harness::launch_with_native_seed().await;
    let admin = connect_admin(&harness).await;
    let created = harness
        .token_with_options(
            "revoke-race",
            TokenMode::ReadOnly,
            3600,
            shamir_connect::server::token::TokenChannels::Both,
            None,
        )
        .await;
    let token = created.token.to_string();
    let addr = harness.addr();

    let baseline_body = ping_body();
    let (baseline_status, _, baseline_response) =
        crate::http_rest::common::request(addr, Some(&token), &baseline_body).await;
    assert_eq!(baseline_status, 200, "REST token baseline ping");
    assert!(matches!(
        rmp_serde::from_slice::<DbResponse>(&baseline_response),
        Ok(DbResponse::Pong)
    ));

    let websocket = harness.token("ws-revoke-race", TokenMode::ReadOnly).await;
    let (mut socket, sid) = open_ws(addr, &websocket).await;
    send_ws_ping(&mut socket, sid, 1)
        .await
        .expect("send baseline websocket ping");
    expect_ws_pong(&mut socket, 1).await;

    let gate = Arc::new(tokio::sync::Barrier::new(WORKERS + 1));
    let ready = Arc::new(tokio::sync::Barrier::new(WORKERS + 1));
    let mut release_senders = Vec::with_capacity(WORKERS);
    let mut tasks = JoinSet::new();
    let revoked = Arc::new(AtomicBool::new(false));
    for _ in 0..WORKERS {
        let token = token.clone();
        let revoked = Arc::clone(&revoked);
        let gate = Arc::clone(&gate);
        let ready = Arc::clone(&ready);
        let (release_sender, release_receiver) = tokio::sync::oneshot::channel();
        release_senders.push(release_sender);
        tasks.spawn(async move {
            tokio::time::timeout(DEADLINE, gate.wait())
                .await
                .expect("worker start barrier timeout");
            let inflight = one_rest_request(
                addr,
                &token,
                &revoked,
                true,
                Some(Arc::clone(&ready)),
                Some(release_receiver),
            )
            .await;
            let post = one_rest_request(addr, &token, &revoked, false, None, None).await;
            (inflight, post)
        });
    }
    tokio::time::timeout(DEADLINE, gate.wait())
        .await
        .expect("worker start barrier timeout");
    tokio::time::timeout(DEADLINE, ready.wait())
        .await
        .expect("in-flight REST readiness timeout");
    admin
        .revoke_api_token(&created.id)
        .await
        .expect("durable token revoke");
    revoked.store(true, Ordering::Release);
    for sender in release_senders {
        sender.send(()).expect("release in-flight REST request");
    }

    let ws_id = token_id(&websocket);
    harness
        .handle
        .token_service
        .revoke(
            ws_id,
            shamir_connect::common::time::UnixNanos::try_now()
                .expect("clock")
                .as_u64(),
        )
        .await
        .expect("revoke websocket token");
    let _ = send_ws_ping(&mut socket, sid, 2).await;
    let deadline = tokio::time::Instant::now() + DEADLINE;
    loop {
        let message = tokio::time::timeout_at(deadline, socket.next())
            .await
            .expect("revoked websocket close timeout")
            .expect("websocket stream")
            .expect("websocket close result");
        match message {
            Message::Close(Some(frame)) => {
                assert_eq!(u16::from(frame.code), 4011);
                break;
            }
            Message::Binary(_) => panic!("successful websocket response after revoke"),
            Message::Ping(_) | Message::Pong(_) => {}
            other => panic!("unexpected websocket frame: {other:?}"),
        }
    }

    let first = tokio::time::timeout(DEADLINE, tasks.join_next()).await;
    let first = match first {
        Ok(Some(result)) => result.expect("REST worker task failed"),
        Ok(None) => panic!("no REST workers were joined"),
        Err(_) => {
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
            panic!("REST workers did not finish before deadline");
        }
    };
    let mut completed = 0;
    let (inflight, post) = first;
    assert!(!inflight.0, "in-flight request started before revoke flag");
    assert_eq!(inflight.1, 401);
    assert!(post.0, "post-request did not start after revoke flag");
    assert_eq!(post.1, 401, "post-flag request must return 401");
    completed += 1;
    while !tasks.is_empty() {
        let next = tokio::time::timeout(DEADLINE, tasks.join_next()).await;
        let result = match next {
            Ok(Some(result)) => result.expect("REST worker task failed"),
            Ok(None) => break,
            Err(_) => {
                tasks.abort_all();
                while tasks.join_next().await.is_some() {}
                panic!("remaining REST workers did not finish before deadline");
            }
        };
        let (inflight, post) = result;
        assert!(!inflight.0, "in-flight request started before revoke flag");
        assert_eq!(inflight.1, 401);
        assert!(post.0, "post-request did not start after revoke flag");
        assert_eq!(post.1, 401, "post-flag request must return 401");
        completed += 1;
    }
    assert_eq!(completed, WORKERS);
    drop(admin);
    drop(websocket);
    assert!(
        tokio::time::timeout(Duration::from_secs(45), harness.handle.shutdown())
            .await
            .expect("shutdown timeout")
            .errors()
            .is_empty()
    );
}
