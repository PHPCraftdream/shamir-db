use super::super::{
    challenge, error, response, success, BeginRequest, Challenge, FinishRequest, Success,
    TokenScope, WireError,
};
use super::fixtures::{begin, encoded_string, fields, pair, scope, valid_begin};
use crate::security::tokens::TokenView;
use serde::Deserialize;
use serde_bytes::ByteBuf;
use shamir_connect::server::token::{TokenChannels, TokenMode};

#[test]
fn begin_accepts_signed_official_shape_and_every_unsigned_width() {
    assert_eq!(
        BeginRequest::parse(&valid_begin()).expect("valid").user,
        "alice"
    );
    for version in [
        vec![1],
        vec![0xcc, 1],
        vec![0xcd, 0, 1],
        vec![0xce, 0, 0, 0, 1],
        vec![0xcf, 0, 0, 0, 0, 0, 0, 0, 1],
    ] {
        let mut data = fields();
        data.version = &version;
        assert!(BeginRequest::parse(&begin(data)).is_ok());
    }
    let mut map16 = vec![0xde, 0, 5];
    map16.extend_from_slice(&valid_begin()[1..]);
    assert!(BeginRequest::parse(&map16).is_ok());
    let mut map32 = vec![0xdf, 0, 0, 0, 5];
    map32.extend_from_slice(&valid_begin()[1..]);
    assert!(BeginRequest::parse(&map32).is_ok());
}

#[test]
fn begin_rejects_bad_outer_and_nested_shapes() {
    let valid = valid_begin();
    let mut trailing = valid.clone();
    trailing.push(0xc0);
    assert_bad_begin(&trailing);
    assert_bad_begin(&valid[..valid.len() - 1]);
    assert_bad_begin(&[0x90]);
    for marker in [0xd0, 0xd1, 0xd2, 0xd3] {
        let mut data = fields();
        let signed = match marker {
            0xd0 => vec![marker, 1],
            0xd1 => vec![marker, 0, 1],
            0xd2 => vec![marker, 0, 0, 0, 1],
            _ => vec![marker, 0, 0, 0, 0, 0, 0, 0, 1],
        };
        data.version = &signed;
        assert_bad_begin(&begin(data));
    }
    let mut duplicate = valid.clone();
    duplicate.extend_from_slice(&valid[1..]);
    duplicate[0] = 0x8a;
    assert_bad_begin(&duplicate);
    let mut unknown = valid.clone();
    let mut extra = Vec::new();
    pair(&mut extra, "unknown", &[1]);
    unknown.push(0);
    unknown.extend_from_slice(&extra);
    unknown[0] = 0x86;
    assert_bad_begin(&unknown);
    for malformed in [
        vec![0x81, 0xa1, b'x', 1],
        vec![0x81, 0xa1, b'x', 0x81, 0xa1, b'y', 0xc0],
    ] {
        assert_bad_begin(&malformed);
    }
}

#[test]
fn begin_rejects_strict_map_field_regressions() {
    let valid = valid_begin();

    let mut unknown = valid.clone();
    let user_key = [0xa4, b'u', b's', b'e', b'r'];
    let at = unknown
        .windows(user_key.len())
        .position(|window| window == user_key)
        .expect("user key");
    unknown[at + 1..at + user_key.len()].copy_from_slice(b"oops");
    assert_bad_begin(&unknown);

    let mut duplicate = valid.clone();
    let binding_key = encoded_string("binding_mode");
    let version_key = encoded_string("protocol_version");
    let at = duplicate
        .windows(binding_key.len())
        .position(|window| window == binding_key)
        .expect("binding key");
    duplicate.splice(at..at + binding_key.len(), version_key);
    assert_bad_begin(&duplicate);

    let mut duplicate_nested = valid.clone();
    let mode_key = encoded_string("mode");
    let label_key = encoded_string("label");
    let at = duplicate_nested
        .windows(mode_key.len())
        .position(|window| window == mode_key)
        .expect("mode key");
    duplicate_nested.splice(at..at + mode_key.len(), label_key);
    assert_bad_begin(&duplicate_nested);

    for (key, replacement) in [
        (encoded_string("protocol_version"), vec![2]),
        (encoded_string("binding_mode"), vec![2]),
    ] {
        let mut bad = valid.clone();
        let at = bad
            .windows(key.len())
            .position(|window| window == key)
            .expect("field key");
        bad[at + key.len()] = replacement[0];
        assert_bad_begin(&bad);
    }

    let mut mistyped_user = valid.clone();
    let at = mistyped_user
        .windows(user_key.len())
        .position(|window| window == user_key)
        .expect("user key");
    mistyped_user.splice(
        at + user_key.len()..at + user_key.len() + 6,
        [0xc4, 4, b'a', b'l', b'i', b'c'],
    );
    assert_bad_begin(&mistyped_user);

    let mut array_nonce = valid.clone();
    let nonce_key = encoded_string("client_nonce");
    let at = array_nonce
        .windows(nonce_key.len())
        .position(|window| window == nonce_key)
        .expect("nonce key");
    let value = at + nonce_key.len();
    array_nonce.splice(
        value..value + 34,
        std::iter::once(0xdc).chain([0, 32]).chain([1; 32]),
    );
    assert_bad_begin(&array_nonce);
}

#[test]
fn begin_checks_bounds_utf8_and_canonical_username() {
    for nonce_len in [31, 33] {
        let mut data = fields();
        let nonce = vec![1; nonce_len];
        data.nonce = &nonce;
        assert_bad_begin(&begin(data));
    }
    let mut data = fields();
    data.nonce = &[0; 32];
    assert_bad_begin(&begin(data));
    for user in ["", &"a".repeat(256), "Alice"] {
        let mut data = fields();
        data.user = user;
        assert_bad_begin(&begin(data));
    }
    let mut data = fields();
    data.user = "é";
    assert_eq!(
        BeginRequest::parse(&begin(data))
            .expect("canonical UTF-8")
            .user,
        "é"
    );
    let mut invalid = valid_begin();
    let at = invalid
        .windows(6)
        .position(|w| w == [0xa5, b'a', b'l', b'i', b'c', b'e'])
        .expect("user");
    invalid[at + 2] = 0xff;
    assert_bad_begin(&invalid);
}

#[test]
fn token_scope_rejects_invalid_fields_and_accepts_boundaries() {
    for label in ["", "x\n", "x\u{7f}", &"x".repeat(129)] {
        assert!(scope(label, None).encode_scope().is_err());
    }
    for databases in [
        vec![],
        vec!["b", "a"],
        vec!["a", "a"],
        vec!["bad!"],
        vec![""],
    ] {
        assert!(scope("x", Some(databases)).encode_scope().is_err());
    }
    assert!(scope("x", Some(vec!["Main", "a-b_9"]))
        .encode_scope()
        .is_ok());
    let max = "x".repeat(128);
    assert_eq!(
        scope(&max, None).encode_scope().expect("label max").len(),
        133
    );
    let a = "a".repeat(251);
    let b = "b".repeat(251);
    assert_eq!(
        scope("x", Some(vec![a.as_str(), b.as_str()]))
            .encode_scope()
            .expect("512 bytes")
            .len(),
        512
    );
    let b = "b".repeat(252);
    assert!(scope("x", Some(vec![a.as_str(), b.as_str()]))
        .encode_scope()
        .is_err());
}

#[test]
fn finish_requires_exact_version_fields_and_binary_lengths() {
    let mut body = vec![0x83];
    pair(&mut body, "protocol_version", &[1]);
    pair(
        &mut body,
        "handshake_id",
        &[0xc4, 32].into_iter().chain([2; 32]).collect::<Vec<_>>(),
    );
    pair(
        &mut body,
        "client_proof",
        &[0xc4, 32].into_iter().chain([3; 32]).collect::<Vec<_>>(),
    );
    let parsed = FinishRequest::parse(&body).expect("valid FINISH");
    assert_eq!(parsed.handshake_id, [2; 32]);
    assert_eq!(parsed.client_proof, [3; 32]);
    assert_bad_finish(&body[..body.len() - 1]);
    let mut trailing = body.clone();
    trailing.push(0xc0);
    assert_bad_finish(&trailing);
    for (len, payload) in [(31, vec![1; 31]), (33, vec![1; 33])] {
        let mut bad = body.clone();
        let at = bad
            .windows(2)
            .position(|w| w == [0xc4, 32])
            .expect("binary");
        bad.splice(at..at + 34, [0xc4, len].into_iter().chain(payload));
        assert_bad_finish(&bad);
    }
    let mut signed = body.clone();
    assert_eq!(signed[18], 1);
    signed.splice(18..19, [0xd0, 1]);
    assert_bad_finish(&signed);
}

#[test]
fn scope_transcript_and_error_response_are_canonical() {
    let encoded = scope("label", Some(vec!["Main", "a-b_9"]))
        .encode_scope()
        .expect("scope");
    assert_eq!(
        TokenScope::from_scope_bytes(&encoded)
            .expect("stored scope")
            .databases
            .as_deref(),
        Some(["Main", "a-b_9"].as_slice())
    );
    assert!(TokenScope::from_scope_bytes(&[0, 1, b'x', 0, 0, 0]).is_err());
    assert_eq!(
        error("rate_limited", Some(7)).expect("body").as_ref(),
        b"\x82\xa5error\xacrate_limited\xabretry_after\x07"
    );
    assert!(response(429, "rate_limited", Some(7)).is_ok());
    assert!(response(503, "server_busy", Some(1)).is_ok());
    assert!(response(400, "invalid_request", None).is_ok());
    for (status, retry) in [
        (400, Some(1)),
        (429, None),
        (503, Some(0)),
        (503, Some(3601)),
    ] {
        assert!(response(status, "invalid_request", retry).is_err());
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectedChallenge {
    protocol_version: u8,
    binding_mode: u8,
    security: u8,
    listener_id: ByteBuf,
    handshake_id: ByteBuf,
    salt: ByteBuf,
    kdf: String,
    memory_kb: u32,
    time: u32,
    parallelism: u32,
    argon2_version: u8,
    server_nonce: ByteBuf,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectedSuccess {
    protocol_version: u8,
    server_signature: ByteBuf,
    token: String,
    id: ByteBuf,
    label: String,
    owner: String,
    mode: String,
    channels: String,
    databases: Option<Vec<String>>,
    created_at_ns: u64,
    expires_at_ns: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectedError {
    error: String,
}

#[test]
fn challenge_success_and_error_messages_have_exact_map_shapes() {
    let challenge_value = Challenge {
        security: 1,
        listener_id: &[1; 16],
        handshake_id: &[2; 32],
        salt: &[3; 16],
        memory_kb: 65536,
        time: 2,
        parallelism: 1,
        server_nonce: &[4; 32],
    };
    let challenge_body = challenge(&challenge_value).expect("challenge");
    let decoded: ExpectedChallenge =
        rmp_serde::from_slice(&challenge_body).expect("exact challenge");
    assert_eq!(
        (
            decoded.protocol_version,
            decoded.binding_mode,
            decoded.security
        ),
        (1, 3, 1)
    );
    assert_eq!(decoded.listener_id.len(), 16);
    assert_eq!(decoded.handshake_id.len(), 32);
    assert_eq!(decoded.salt.len(), 16);
    assert_eq!(decoded.server_nonce.len(), 32);
    assert_eq!(
        (
            decoded.kdf.as_str(),
            decoded.memory_kb,
            decoded.time,
            decoded.parallelism,
            decoded.argon2_version
        ),
        ("argon2id", 65536, 2, 1, 19)
    );
    let view = TokenView {
        id: [5; 16],
        owner_user_id: [6; 16],
        label: "http-login".into(),
        created_at_ns: 1,
        expires_at_ns: 2,
        mode: TokenMode::ReadOnly,
        channels: TokenChannels::Rest,
        databases: None,
        revoked_at_ns: None,
    };
    let success_body = success(&Success {
        server_signature: &[7; 32],
        token: "bearer",
        view: &view,
        owner: "alice",
    })
    .expect("success");
    let decoded: ExpectedSuccess = rmp_serde::from_slice(&success_body).expect("exact success");
    assert_eq!(
        (
            decoded.protocol_version,
            decoded.token.as_str(),
            decoded.label.as_str(),
            decoded.owner.as_str(),
            decoded.mode.as_str(),
            decoded.channels.as_str()
        ),
        (1, "bearer", "http-login", "alice", "read_only", "rest")
    );
    assert_eq!(decoded.server_signature.len(), 32);
    assert_eq!(decoded.id.len(), 16);
    assert_eq!(decoded.databases, None);
    assert_eq!((decoded.created_at_ns, decoded.expires_at_ns), (1, 2));
    let decoded_error: ExpectedError =
        rmp_serde::from_slice(&error("invalid_request", None).expect("error"))
            .expect("exact error");
    assert_eq!(decoded_error.error, "invalid_request");
    assert_eq!(encoded_string("a"), [0xa1, b'a']);
}

fn assert_bad_begin(body: &[u8]) {
    assert_eq!(BeginRequest::parse(body).err(), Some(WireError::Invalid));
}
fn assert_bad_finish(body: &[u8]) {
    assert_eq!(FinishRequest::parse(body).err(), Some(WireError::Invalid));
}
