use std::time::Duration;

use super::super::http_rest::common::Harness;
use super::common::{raw_response, upgrade};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use shamir_connect::server::token::{parse_token, TokenMode};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};

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
            config.tokens.max_sockets_per_token = 1;
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
    let no_binding = tokio::time::timeout(Duration::from_millis(100), socket.next()).await;
    assert!(
        no_binding.is_err(),
        "server sent a frame before credentials"
    );

    let token_bytes = rmp_serde::to_vec_named(&TokenFrame { token: &token }).expect("encode token");
    send_frame(&mut socket, token_bytes).await;
    let message = tokio::time::timeout(Duration::from_secs(15), socket.next())
        .await
        .expect("binding timeout")
        .expect("binding frame")
        .expect("binding message");
    let Message::Binary(bytes) = message else {
        panic!("binding was not binary");
    };
    let binding: Binding = rmp_serde::from_slice(&bytes).expect("binding map");
    assert!(binding.ok && binding.expires_at_ns > 0 && binding.sid.len() == 32);
    assert!(binding.sid.iter().any(|byte| *byte != 0));
    close_cleanly(socket).await;

    browser_refusal_matrix(harness).await;
    browser_first_frame_failures(harness, &token).await;
    browser_canonical_credential_failures(harness, &token).await;
    browser_cap_case(harness, &token).await;
}

#[derive(serde::Serialize)]
struct TokenFrame<'a> {
    token: &'a str,
}

async fn send_frame(socket: &mut WebSocketStream<TcpStream>, frame: Vec<u8>) {
    tokio::time::timeout(
        Duration::from_secs(15),
        socket.send(Message::Binary(frame.into())),
    )
    .await
    .expect("frame write timeout")
    .expect("send frame");
}

async fn close_cleanly(mut socket: WebSocketStream<TcpStream>) {
    tokio::time::timeout(Duration::from_secs(5), async {
        socket.close(None).await.expect("send close");
        match socket.next().await {
            Some(Ok(Message::Close(_))) | None => {}
            Some(Err(tokio_tungstenite::tungstenite::Error::ConnectionClosed))
            | Some(Err(tokio_tungstenite::tungstenite::Error::AlreadyClosed)) => {}
            Some(Err(error)) => panic!("server close failed: {error}"),
            Some(Ok(message)) => panic!("unexpected message after close: {message:?}"),
        }
    })
    .await
    .expect("server close timeout");
}

async fn browser_cap_case(harness: &Harness, token: &str) {
    let mut first = browser_socket(harness.addr()).await;
    send_frame(
        &mut first,
        rmp_serde::to_vec_named(&TokenFrame { token }).expect("encode token"),
    )
    .await;
    let binding = tokio::time::timeout(Duration::from_secs(15), first.next())
        .await
        .expect("first binding timeout")
        .expect("first binding frame")
        .expect("first binding message");
    assert!(matches!(binding, Message::Binary(_)));

    let mut second = browser_socket(harness.addr()).await;
    send_frame(
        &mut second,
        rmp_serde::to_vec_named(&TokenFrame { token }).expect("encode token"),
    )
    .await;
    assert_close_code(&mut second, 1013).await;
    close_cleanly(first).await;
}

async fn browser_canonical_credential_failures(harness: &Harness, valid: &str) {
    let mut wrong_secret = valid.as_bytes().to_vec();
    wrong_secret[28] = if wrong_secret[28] == b'A' { b'B' } else { b'A' };
    let wrong_secret = String::from_utf8(wrong_secret).expect("token remains ASCII");
    assert!(parse_token(wrong_secret.as_bytes()).is_ok());

    let unknown = "shk1.AAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    assert!(parse_token(unknown.as_bytes()).is_ok());

    let revoked = harness.token("browser-revoked", TokenMode::ReadOnly).await;
    let revoked_id = parse_token(revoked.as_bytes())
        .expect("issued token parses")
        .id;
    assert!(harness
        .handle
        .token_service
        .revoke(
            revoked_id,
            shamir_connect::common::time::UnixNanos::try_now()
                .expect("clock")
                .as_u64(),
        )
        .await
        .expect("revoke token"));

    let expired = harness
        .token_with_options(
            "browser-expired",
            TokenMode::ReadOnly,
            1,
            shamir_connect::server::token::TokenChannels::Both,
            None,
        )
        .await;
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let now_ns = shamir_connect::common::time::UnixNanos::try_now()
                .expect("clock")
                .as_u64();
            if now_ns >= expired.record_view.expires_at_ns {
                break;
            }
            tokio::time::sleep(Duration::from_nanos(
                expired.record_view.expires_at_ns - now_ns,
            ))
            .await;
        }
    })
    .await
    .expect("expired token deadline timeout");

    for candidate in [
        unknown,
        wrong_secret.as_str(),
        revoked.as_str(),
        expired.token.as_str(),
    ] {
        assert!(parse_token(candidate.as_bytes()).is_ok());
        let mut socket = browser_socket(harness.addr()).await;
        send_frame(
            &mut socket,
            rmp_serde::to_vec_named(&TokenFrame { token: candidate })
                .expect("encode canonical token"),
        )
        .await;
        assert_close_code(&mut socket, 4000).await;
    }
}

async fn browser_first_frame_failures(harness: &Harness, token: &str) {
    let invalid = "invalid-token-without-secret-material";
    let rest_only = harness
        .token_with_channels(
            "browser-rest-only",
            shamir_connect::server::token::TokenMode::ReadOnly,
            shamir_connect::server::token::TokenChannels::Rest,
        )
        .await;
    let rest_only_frame =
        rmp_serde::to_vec_named(&TokenFrame { token: &rest_only }).expect("encode rest-only token");
    let mut trailing = rmp_serde::to_vec_named(&TokenFrame { token }).expect("encode token");
    trailing.push(0);
    let mut malformed = vec![0x81, 0xc4, 5, b't', b'o', b'k', b'e', b'n', 0xd9, 71];
    malformed.extend(std::iter::repeat_n(b'a', 71));
    let cases = [
        (Some(masked_frame(1, true, b"x")), 4000),
        (Some(masked_frame(2, true, &vec![0; 257])), 4000),
        (
            Some(masked_frame(
                2,
                true,
                &rmp_serde::to_vec_named(&TokenFrame { token: invalid })
                    .expect("encode invalid token"),
            )),
            4000,
        ),
        (Some(masked_frame(2, true, &rest_only_frame)), 4013),
        (Some(masked_frame(2, true, &trailing)), 4000),
        (Some(masked_frame(2, true, &malformed)), 4000),
        (None, 4000),
    ];
    for (frame, code) in cases {
        let mut socket = browser_socket(harness.addr()).await;
        if let Some(frame) = frame {
            socket
                .get_mut()
                .write_all(&frame)
                .await
                .expect("send raw test frame");
        }
        assert_close_code(&mut socket, code).await;
    }
}

fn masked_frame(opcode: u8, fin: bool, payload: &[u8]) -> Vec<u8> {
    assert!(payload.len() <= u16::MAX as usize);
    let mask = [3_u8, 7, 11, 19];
    let mut frame = vec![(if fin { 0x80 } else { 0 }) | opcode];
    match payload.len() {
        0..=125 => frame.push(0x80 | payload.len() as u8),
        126..=65535 => {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        }
        _ => unreachable!("payload length checked above"),
    }
    frame.extend_from_slice(&mask);
    frame.extend(
        payload
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ mask[index % 4]),
    );
    frame
}

async fn assert_close_code(socket: &mut WebSocketStream<TcpStream>, expected: u16) {
    let message = tokio::time::timeout(Duration::from_secs(15), socket.next())
        .await
        .expect("close timeout")
        .expect("close frame")
        .expect("close message");
    let Message::Close(Some(frame)) = message else {
        panic!("expected close frame");
    };
    assert_eq!(u16::from(frame.code), expected);
    if expected == 4000 {
        assert_eq!(frame.reason, "authentication_failed");
    } else if expected == 1013 {
        assert_eq!(frame.reason, "server_busy");
    } else if expected == 4013 {
        assert_eq!(frame.reason, "token_channel_denied");
    }
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
    let requests = [
        (
            upgrade("/shamir/v1/ws/browser", "", Some("https://app.example"), ""),
            "400",
        ),
        (
            upgrade(
                "/shamir/v1/ws/browser",
                "bad host",
                Some("https://app.example"),
                "",
            ),
            "400",
        ),
        (
            upgrade(
                "/shamir/v1/ws/browser",
                "evil.example",
                Some("https://app.example"),
                "",
            ),
            "400",
        ),
        (
            upgrade(
                "/shamir/v1/ws/browser",
                "localhost",
                Some("https://app.example/path"),
                "",
            ),
            "400",
        ),
        (
            upgrade(
                "/shamir/v1/ws/browser",
                "localhost",
                Some("https://app.example"),
                "Origin: https://app.example\r\n",
            ),
            "400",
        ),
        (
            upgrade(
                "/shamir/v1/ws/browser",
                "localhost",
                Some("https://app.example"),
                "Host: localhost\r\n",
            ),
            "400",
        ),
        (
            upgrade("/shamir/v1/ws/browser", "localhost", None, ""),
            "400",
        ),
        (
            upgrade(
                "/shamir/v1/ws/browser",
                "localhost",
                Some("https://evil.example"),
                "",
            ),
            "403",
        ),
    ];
    for (request, status) in requests {
        let (headers, _) = raw_response(harness.addr(), &request).await;
        assert_eq!(headers.split_whitespace().nth(1), Some(status), "{headers}");
    }
}
