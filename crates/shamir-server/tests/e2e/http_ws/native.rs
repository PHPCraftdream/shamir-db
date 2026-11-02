use super::super::http_rest::common::Harness;
use super::common::{raw_response, upgrade};
use shamir_connect::server::token::TokenMode;

pub(super) async fn exercise_native_group(harness: &Harness) {
    let token = harness.token("ws-native", TokenMode::ReadOnly).await;
    let valid = upgrade(
        "/shamir/v1/ws",
        "localhost",
        None,
        &format!("Authorization: Bearer {token}\r\n"),
    );
    let (headers, _) = raw_response(harness.addr(), &valid).await;
    assert_eq!(headers.split_whitespace().nth(1), Some("101"), "{headers}");
    let malformed_auth = upgrade(
        "/shamir/v1/ws",
        "localhost",
        None,
        "Authorization: Bearer malformed\r\n",
    );
    let (malformed, _) = raw_response(harness.addr(), &malformed_auth).await;
    assert_eq!(
        malformed.split_whitespace().nth(1),
        Some("401"),
        "{malformed}"
    );
    let missing = upgrade("/shamir/v1/ws", "localhost", None, "");
    let duplicate_auth = upgrade(
        "/shamir/v1/ws",
        "localhost",
        None,
        &format!("Authorization: Bearer {token}\r\nAuthorization: Bearer {token}\r\n"),
    );
    let (duplicate, _) = raw_response(harness.addr(), &duplicate_auth).await;
    assert_eq!(
        duplicate.split_whitespace().nth(1),
        Some("401"),
        "{duplicate}"
    );
    let (first, first_body) = raw_response(harness.addr(), &missing).await;
    let (second, second_body) = raw_response(harness.addr(), &missing).await;
    assert_eq!(first.split_whitespace().nth(1), Some("401"), "{first}");
    assert_eq!(second.split_whitespace().nth(1), Some("401"), "{second}");
    assert_eq!(first_body, second_body);
    assert_eq!(
        first
            .lines()
            .filter(|line| !line.to_ascii_lowercase().starts_with("date:"))
            .collect::<Vec<_>>(),
        second
            .lines()
            .filter(|line| !line.to_ascii_lowercase().starts_with("date:"))
            .collect::<Vec<_>>()
    );

    let case_token = harness.token("ws-case", TokenMode::ReadOnly).await;
    let invalid_origin = upgrade(
        "/shamir/v1/ws",
        "localhost",
        Some("https://forbidden.example"),
        &format!("Authorization: Bearer {case_token}\r\n"),
    );
    let (headers, _) = raw_response(harness.addr(), &invalid_origin).await;
    assert_eq!(headers.split_whitespace().nth(1), Some("403"), "{headers}");
    let malformed_protocol = "GET /shamir/v1/ws HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Protocol: wrong\r\n\r\n";
    let (headers, _) = raw_response(harness.addr(), malformed_protocol).await;
    assert_eq!(headers.split_whitespace().nth(1), Some("400"), "{headers}");
    let bad_key = "GET /shamir/v1/ws HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: bad\r\nSec-WebSocket-Protocol: shamir-v1\r\n\r\n";
    let (headers, _) = raw_response(harness.addr(), bad_key).await;
    assert_eq!(headers.split_whitespace().nth(1), Some("400"), "{headers}");
    let wrong_method = valid.replacen("GET", "POST", 1);
    let (headers, _) = raw_response(harness.addr(), &wrong_method).await;
    assert_eq!(headers.split_whitespace().nth(1), Some("405"), "{headers}");
    assert!(headers.to_ascii_lowercase().contains("allow: get"));

    let rest_only = harness
        .token_with_channels(
            "ws-rest-only",
            TokenMode::ReadOnly,
            shamir_connect::server::token::TokenChannels::Rest,
        )
        .await;
    let denied = upgrade(
        "/shamir/v1/ws",
        "localhost",
        None,
        &format!("Authorization: Bearer {rest_only}\r\n"),
    );
    let (headers, body) = raw_response(harness.addr(), &denied).await;
    assert_eq!(headers.split_whitespace().nth(1), Some("403"), "{headers}");
    let response: shamir_query_types::wire::DbResponse =
        rmp_serde::from_slice(&body).expect("decode channel denial");
    assert!(
        matches!(response, shamir_query_types::wire::DbResponse::Error { ref code, ref message } if code == "token_channel_denied" && message == "token_channel_denied")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_ws_native_group() {
    let harness = Harness::launch_with_native_seed_hook(
        "127.0.0.1:0",
        shamir_server::config::HttpListenerSettings {
            tls: Some(false),
            ..Default::default()
        },
        |config| {
            config.tokens.max_sockets_per_token = 2;
            config.tokens.max_sockets_per_owner = 3;
        },
    )
    .await;
    exercise_native_group(&harness).await;
    super::session_loop::exercise_native_session(&harness).await;
    let report = harness.handle.shutdown().await;
    assert!(report.errors().is_empty(), "{report:?}");
}
