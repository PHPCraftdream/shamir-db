use crate::http_rest::common::Harness;
use crate::token_matrix::support::{connect, send_http, status, DEADLINE, MAX_HEAD};
use futures_util::StreamExt;
use shamir_connect::common::time::UnixNanos;
use shamir_connect::server::token::{parse_token, Endpoint, TokenMode, VerifyFailure};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, http::HeaderValue, Message};

async fn framing(harness: &Harness, tls: bool) {
    let token = harness
        .token(
            "framing",
            shamir_connect::server::token::TokenMode::ReadOnly,
        )
        .await;
    let flood = format!("POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nContent-Type: application/msgpack\r\nX-Flood: {}\r\nContent-Length: 0\r\n\r\n", "x".repeat(MAX_HEAD + 1024));
    let (head, _) = send_http(harness, tls, &flood).await;
    assert_eq!(status(&head), 431);

    let token = harness
        .token(
            "framing-slow",
            shamir_connect::server::token::TokenMode::ReadOnly,
        )
        .await;
    let mut stream = connect(harness, tls).await;
    let partial = format!("POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nContent-Type: application/msgpack\r\nContent-Length: 1\r\n");
    tokio::time::timeout(DEADLINE, stream.write_all(partial.as_bytes()))
        .await
        .expect("partial head write timeout")
        .expect("partial head write");
    let (head, _) = crate::token_matrix::support::read_response(&mut stream).await;
    assert_eq!(status(&head), 408);
    let mut byte = [0; 1];
    let n = tokio::time::timeout(DEADLINE, stream.read(&mut byte))
        .await
        .expect("EOF timeout")
        .expect("read EOF");
    assert_eq!(n, 0);

    let token = harness
        .token(
            "framing-oversized",
            shamir_connect::server::token::TokenMode::ReadOnly,
        )
        .await;
    let mut stream = connect(harness, tls).await;
    let request = format!("POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nContent-Type: application/msgpack\r\nContent-Length: 16777217\r\nConnection: close\r\n\r\n");
    tokio::time::timeout(DEADLINE, stream.write_all(request.as_bytes()))
        .await
        .expect("oversized request write timeout")
        .expect("oversized request head");
    let (head, _) = crate::token_matrix::support::read_response(&mut stream).await;
    assert_eq!(status(&head), 413);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_channel_header_flood_slow_head_and_oversize_are_bounded() {
    let harness = Harness::launch().await;
    framing(&harness, false).await;
    assert!(harness.handle.shutdown().await.errors().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn https_channel_header_flood_slow_head_and_oversize_are_bounded() {
    let harness = Harness::launch_with(
        "127.0.0.1:0",
        shamir_server::config::HttpListenerSettings {
            tls: Some(true),
            ..Default::default()
        },
    )
    .await;
    framing(&harness, true).await;
    assert!(harness.handle.shutdown().await.errors().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wss_oversized_frame_and_revoke_closure() {
    let harness = Harness::launch_with(
        "127.0.0.1:0",
        shamir_server::config::HttpListenerSettings {
            tls: Some(true),
            ..Default::default()
        },
    )
    .await;

    let oversized_token = harness.token("wss-frame", TokenMode::ReadOnly).await;
    let mut request = format!("wss://{}/shamir/v1/ws", harness.addr())
        .into_client_request()
        .expect("WSS request");
    request.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {oversized_token}")).expect("authorization"),
    );
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static("shamir-v1"),
    );
    let (mut ws, response) = tokio::time::timeout(
        DEADLINE,
        tokio_tungstenite::client_async(request, connect(&harness, true).await),
    )
    .await
    .expect("WSS handshake timeout")
    .expect("WSS handshake");
    assert_eq!(response.status().as_u16(), 101);
    assert!(matches!(
        tokio::time::timeout(DEADLINE, ws.next())
            .await
            .expect("binding timeout")
            .expect("binding frame")
            .expect("binding message"),
        Message::Binary(_)
    ));
    let mut frame = Vec::with_capacity(14);
    frame.extend_from_slice(&[0x82, 0xff]);
    frame.extend_from_slice(&(16u64 * 1024 * 1024 + 1).to_be_bytes());
    frame.extend_from_slice(&[1, 2, 3, 4]);
    tokio::time::timeout(DEADLINE, ws.get_mut().write_all(&frame))
        .await
        .expect("oversized frame write timeout")
        .expect("oversized frame header write");
    let message = tokio::time::timeout(DEADLINE, ws.next())
        .await
        .expect("oversized close timeout")
        .expect("oversized close stream")
        .expect("oversized close frame");
    let Message::Close(Some(close)) = message else {
        panic!("expected oversized-frame close, got {message:?}");
    };
    assert_eq!(u16::from(close.code), 1009);
    drop(ws);

    let revoke_token = harness.token("wss-revoke", TokenMode::ReadOnly).await;
    let parsed = parse_token(revoke_token.as_bytes()).expect("issued token parses");
    let mut request = format!("wss://{}/shamir/v1/ws", harness.addr())
        .into_client_request()
        .expect("WSS request");
    request.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {revoke_token}")).expect("authorization"),
    );
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static("shamir-v1"),
    );
    let (mut ws, response) = tokio::time::timeout(
        DEADLINE,
        tokio_tungstenite::client_async(request, connect(&harness, true).await),
    )
    .await
    .expect("WSS handshake timeout")
    .expect("WSS handshake");
    assert_eq!(response.status().as_u16(), 101);
    assert!(matches!(
        tokio::time::timeout(DEADLINE, ws.next())
            .await
            .expect("binding timeout")
            .expect("binding frame")
            .expect("binding message"),
        Message::Binary(_)
    ));
    assert!(
        harness
            .handle
            .token_service
            .sessions()
            .count_for_token(&parsed.id)
            > 0
    );
    harness
        .handle
        .token_service
        .revoke(parsed.id, UnixNanos::try_now().expect("clock").as_u64())
        .await
        .expect("revoke token");
    let message = tokio::time::timeout(DEADLINE, ws.next())
        .await
        .expect("revocation close timeout")
        .expect("revocation close stream")
        .expect("revocation close frame");
    let Message::Close(Some(close)) = message else {
        panic!("expected revocation close, got {message:?}");
    };
    assert_eq!(u16::from(close.code), 4011);
    assert_eq!(
        harness
            .handle
            .token_service
            .sessions()
            .count_for_token(&parsed.id),
        0
    );
    drop(ws);

    let request = format!("GET /shamir/v1/ping HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {revoke_token}\r\nContent-Type: application/msgpack\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let (head, _) = send_http(&harness, true, &request).await;
    assert_eq!(status(&head), 401);

    let report = tokio::time::timeout(
        std::time::Duration::from_secs(45),
        harness.handle.shutdown(),
    )
    .await
    .expect("shutdown timeout");
    assert!(report.errors().is_empty(), "shutdown errors: {report:?}");
}

async fn tls_auth_gap(endpoint: &str, case: &str) {
    let harness = Harness::launch_with(
        "127.0.0.1:0",
        shamir_server::config::HttpListenerSettings {
            tls: Some(true),
            ..Default::default()
        },
    )
    .await;
    let token = harness.token("tls-auth-gap", TokenMode::ReadOnly).await;
    let parsed = parse_token(token.as_bytes()).expect("issued token parses");
    let credential = match case {
        "wrong-secret" => {
            let mut bytes = token.as_bytes().to_vec();
            bytes[28] = if bytes[28] == b'A' { b'B' } else { b'A' };
            String::from_utf8(bytes).expect("mutated token")
        }
        "unknown" => {
            "shk1.AAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_owned()
        }
        "expired" => {
            let mut record = harness
                .handle
                .token_service
                .mirror()
                .get(&parsed.id)
                .expect("token record")
                .record()
                .clone();
            record.created_at_ns = 1;
            record.expires_at_ns = 2;
            assert!(harness.handle.token_service.mirror().remove(&parsed.id));
            assert!(harness.handle.token_service.mirror().insert(record));
            let now = UnixNanos::try_now().expect("clock").as_u64();
            assert!(matches!(
                harness
                    .handle
                    .token_service
                    .verify(token.as_bytes(), Endpoint::Rest, now),
                Err(VerifyFailure::Expired)
            ));
            token.clone()
        }
        "revoked" => {
            harness
                .handle
                .token_service
                .revoke(parsed.id, UnixNanos::try_now().expect("clock").as_u64())
                .await
                .expect("revoke");
            token.clone()
        }
        _ => token.clone(),
    };
    if case == "unknown" {
        let id = parse_token(credential.as_bytes()).expect("unknown parses");
        assert!(harness.handle.token_service.mirror().get(&id.id).is_none());
    }
    if endpoint == "rest" {
        let auth = match case {
            "missing" => String::new(),
            "duplicate" => {
                format!("Authorization: Bearer {token}\r\nAuthorization: Bearer {token}\r\n")
            }
            _ => format!("Authorization: Bearer {credential}\r\n"),
        };
        let req=format!("GET /shamir/v1/ping HTTP/1.1\r\nHost: localhost\r\n{auth}Content-Type: application/msgpack\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        let (head, body) = send_http(&harness, true, &req).await;
        assert_eq!(status(&head), 401, "{case}");
        assert_eq!(body, b"\x81\xa5error\xacunauthorized", "{case}");
        let h = head.to_ascii_lowercase();
        assert!(h.lines().any(|x| x.trim() == "www-authenticate: bearer"));
        assert!(h.lines().any(|x| x.trim() == "connection: close"));
    } else {
        let auth = match case {
            "missing" => String::new(),
            "duplicate" => {
                format!("Authorization: Bearer {token}\r\nAuthorization: Bearer {token}\r\n")
            }
            _ => format!("Authorization: Bearer {credential}\r\n"),
        };
        let req = format!("GET /shamir/v1/ws HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Protocol: shamir-v1\r\n{auth}\r\n");
        let (head, body) = send_http(&harness, true, &req).await;
        assert_eq!(status(&head), 401, "{case}");
        assert_eq!(body, b"\x81\xa5error\xacunauthorized", "{case}");
        let h = head.to_ascii_lowercase();
        assert!(h.lines().any(|x| x.trim() == "www-authenticate: bearer"));
        assert!(h.lines().any(|x| x.trim() == "connection: close"));
    }
    let report = tokio::time::timeout(
        std::time::Duration::from_secs(45),
        harness.handle.shutdown(),
    )
    .await
    .expect("shutdown timeout");
    assert!(report.errors().is_empty(), "{case}: {report:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tls_rest_auth_gap_missing() {
    tls_auth_gap("rest", "missing").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tls_rest_auth_gap_duplicate() {
    tls_auth_gap("rest", "duplicate").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tls_rest_auth_gap_expired() {
    tls_auth_gap("rest", "expired").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tls_rest_auth_gap_revoked() {
    tls_auth_gap("rest", "revoked").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tls_wss_auth_gap_missing() {
    tls_auth_gap("wss", "missing").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tls_wss_auth_gap_duplicate() {
    tls_auth_gap("wss", "duplicate").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tls_wss_auth_gap_wrong_secret() {
    tls_auth_gap("wss", "wrong-secret").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tls_wss_auth_gap_unknown() {
    tls_auth_gap("wss", "unknown").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tls_wss_auth_gap_expired() {
    tls_auth_gap("wss", "expired").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tls_wss_auth_gap_revoked() {
    tls_auth_gap("wss", "revoked").await;
}
