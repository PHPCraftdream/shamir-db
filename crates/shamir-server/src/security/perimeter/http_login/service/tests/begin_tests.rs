use super::super::begin;
use super::fixture::Fixture;
use crate::http_login::admission::Source;
use crate::http_login::wire::{response, TokenScopeMode};
use axum::http::StatusCode;
use serde::Deserialize;
use shamir_connect::common::fake_blob::FakeBlob;
use shamir_connect::common::username::NormalizedUsername;
use shamir_connect::server::lockout::LockoutStore;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

fn source() -> Source {
    Source::from_ip(IpAddr::V4(Ipv4Addr::LOCALHOST))
}

#[tokio::test(start_paused = true)]
async fn malformed_begin_is_padded_and_has_no_side_effects() {
    let fixture = Fixture::new().await;
    let started = tokio::time::Instant::now();
    let result = begin(&fixture.context, source(), &[]).await;
    let elapsed = tokio::time::Instant::now() - started;

    assert_eq!(result.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        result.body(),
        response(400, "invalid_request", None)
            .expect("response")
            .body()
    );
    assert!(elapsed >= Duration::from_millis(50));
    assert!(elapsed <= Duration::from_millis(75));
    assert_eq!(fixture.context.pending.count().await, 0);
    assert_eq!(fixture.lockout.failure_pair_count(), 0);
    assert_eq!(
        fixture
            .lockout
            .current_backoff_ms(([0; 16], [0; 16]), 90_000_000_000),
        0
    );
    assert_eq!(fixture.context.tokens.mirror_entry_count_hint(), 0);
}

#[tokio::test(start_paused = true)]
async fn begin_policy_denials_are_padded_and_have_no_side_effects() {
    for (read_write, self_service, read_only, mode, expected) in [
        (
            false,
            true,
            false,
            TokenScopeMode::ReadWrite,
            "login_write_forbidden",
        ),
        (
            true,
            false,
            false,
            TokenScopeMode::ReadOnly,
            "token_self_service_disabled",
        ),
        (
            true,
            true,
            true,
            TokenScopeMode::ReadOnly,
            "login_read_only_replica",
        ),
    ] {
        let mut fixture = Fixture::new().await;
        fixture.context.login_allow_read_write = read_write;
        fixture.context.self_service = self_service;
        fixture.context.read_only = read_only;
        let started = tokio::time::Instant::now();
        let body = valid_begin(mode);
        let result = begin(&fixture.context, source(), &body).await;
        let elapsed = tokio::time::Instant::now() - started;

        assert_eq!(result.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            result.body(),
            response(403, expected, None).expect("response").body()
        );
        assert!(elapsed >= Duration::from_millis(50));
        assert!(elapsed <= Duration::from_millis(75));
        assert_eq!(fixture.context.pending.count().await, 0);
        assert_eq!(fixture.lockout.failure_pair_count(), 0);
        assert_eq!(
            fixture
                .lockout
                .current_backoff_ms(([0; 16], [0; 16]), 90_000_000_000),
            0
        );
        assert_eq!(fixture.context.tokens.mirror_entry_count_hint(), 0);
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Challenge {
    protocol_version: u8,
    binding_mode: u8,
    security: u8,
    listener_id: serde_bytes::ByteBuf,
    handshake_id: serde_bytes::ByteBuf,
    salt: serde_bytes::ByteBuf,
    kdf: String,
    memory_kb: u32,
    time: u32,
    parallelism: u32,
    argon2_version: u8,
    server_nonce: serde_bytes::ByteBuf,
}

#[tokio::test(start_paused = true)]
async fn known_and_unknown_users_receive_indistinguishable_challenges() {
    let mut unknown_salt = Vec::new();
    let mut challenge_body_len = None;
    for user in ["alice", "nobody"] {
        let fixture = Fixture::new().await;
        let started = tokio::time::Instant::now();
        let mut body = valid_begin(TokenScopeMode::ReadOnly);
        // The encoded username is the second map value; replace its one-byte length and bytes.
        let user_pair = encoded_string("user");
        let value_start = body
            .windows(user_pair.len())
            .position(|window| window == user_pair)
            .expect("user key")
            + user_pair.len();
        let old = encoded_string("alice");
        assert_eq!(&body[value_start..value_start + old.len()], old);
        body.splice(value_start..value_start + old.len(), encoded_string(user));
        let result = begin(&fixture.context, source(), &body).await;
        let elapsed = tokio::time::Instant::now() - started;
        assert!(elapsed >= Duration::from_millis(50));
        assert!(elapsed <= Duration::from_millis(75));
        assert_eq!(result.status(), StatusCode::OK);
        let body_len = result.body().len();
        if let Some(expected_len) = challenge_body_len {
            assert_eq!(body_len, expected_len);
        } else {
            challenge_body_len = Some(body_len);
        }
        let challenge: Challenge = rmp_serde::from_slice(result.body()).expect("challenge schema");
        assert_eq!(challenge.protocol_version, 1);
        assert_eq!(challenge.binding_mode, 3);
        assert_eq!(challenge.security, 0);
        assert_eq!(challenge.listener_id.as_ref(), &[3; 16]);
        assert_eq!(
            (
                challenge.kdf.as_str(),
                challenge.memory_kb,
                challenge.time,
                challenge.parallelism,
                challenge.argon2_version
            ),
            ("argon2id", 19_456, 2, 1, 0x13)
        );
        assert_eq!(challenge.handshake_id.len(), 32);
        assert_eq!(challenge.server_nonce.len(), 32);
        assert_eq!(fixture.context.pending.count().await, 1);
        if user == "nobody" {
            let normalized =
                NormalizedUsername::from_raw(user).expect("normalized fixture username");
            let fake = FakeBlob::derive(&fixture.context.secrets.server_secret, &normalized)
                .expect("fake verifier");
            assert_eq!(challenge.salt.as_ref(), fake.salt);
            unknown_salt = challenge.salt.into_vec();
        } else {
            assert_eq!(challenge.salt.as_ref(), &[0x42; 16]);
            assert_eq!(fixture.context.pending.count().await, 1);
        }
    }
    assert_ne!(unknown_salt.as_slice(), &[0x42; 16]);
}

fn valid_begin(mode: TokenScopeMode) -> Vec<u8> {
    let mode = match mode {
        TokenScopeMode::ReadOnly => "read_only",
        TokenScopeMode::ReadWrite => "read_write",
    };
    let mut scope = vec![0x84];
    pair(&mut scope, "label", &encoded_string("http-login"));
    pair(&mut scope, "mode", &encoded_string(mode));
    pair(&mut scope, "channels", &encoded_string("rest"));
    pair(&mut scope, "databases", &[0xc0]);
    let mut body = vec![0x85];
    pair(&mut body, "protocol_version", &[1]);
    pair(&mut body, "user", &encoded_string("alice"));
    pair(
        &mut body,
        "client_nonce",
        &[0xc4, 32].into_iter().chain([1; 32]).collect::<Vec<_>>(),
    );
    pair(&mut body, "binding_mode", &[3]);
    pair(&mut body, "token", &scope);
    body
}

fn encoded_string(value: &str) -> Vec<u8> {
    let mut encoded = vec![0xa0 | u8::try_from(value.len()).expect("short fixture string")];
    encoded.extend_from_slice(value.as_bytes());
    encoded
}

fn pair(body: &mut Vec<u8>, key: &str, value: &[u8]) {
    body.extend_from_slice(&encoded_string(key));
    body.extend_from_slice(value);
}

#[tokio::test(start_paused = true)]
async fn hardened_mismatched_tuple_uses_fake_salt_and_current_default() {
    let mut fixture = Fixture::new().await;
    fixture.context.hardening = true;
    fixture.context.default_kdf.memory_kb = 19_457;
    let body = valid_begin(TokenScopeMode::ReadOnly);
    let result = begin(&fixture.context, source(), &body).await;
    assert_eq!(result.status(), StatusCode::OK);
    let challenge: Challenge = rmp_serde::from_slice(result.body()).expect("challenge");
    let user = NormalizedUsername::from_raw("alice").expect("username");
    let fake = FakeBlob::derive(&fixture.context.secrets.server_secret, &user).expect("fake");
    assert_eq!(challenge.salt.as_ref(), fake.salt.as_slice());
    assert_ne!(challenge.salt.as_ref(), [0x42; 16].as_slice());
    assert_eq!(challenge.memory_kb, 19_457);
    assert_eq!(fixture.context.pending.count().await, 1);
    assert_eq!(fixture.lockout.failure_pair_count(), 0);
}
