//! Login challenge / success decoding and begin / finish encoding.

use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;
use shamir_connect::server::token::TokenId;

use crate::client::handshake::token::{
    decode_challenge, decode_finish, encode_begin, encode_finish,
};
use crate::client::{LoginChannels, LoginMode, LoginScope};
use crate::error::ClientError;

/// A field that may be a msgpack bin, a str, or an array of ints.
#[derive(Serialize, Clone)]
#[serde(untagged)]
enum Blob {
    Bin(#[serde(with = "serde_bytes")] Vec<u8>),
    Str(String),
    Arr(Vec<u8>),
}

impl Blob {
    fn bin(len: usize, fill: u8) -> Self {
        Self::Bin(vec![fill; len])
    }
}

/// Challenge map in which any key can be omitted or replaced.
#[derive(Serialize, Clone)]
struct ChallengeWire {
    #[serde(skip_serializing_if = "Option::is_none")]
    protocol_version: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    binding_mode: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    security: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    listener_id: Option<Blob>,
    #[serde(skip_serializing_if = "Option::is_none")]
    handshake_id: Option<Blob>,
    #[serde(skip_serializing_if = "Option::is_none")]
    salt: Option<Blob>,
    #[serde(skip_serializing_if = "Option::is_none")]
    kdf: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    memory_kb: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    time: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parallelism: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    argon2_version: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    server_nonce: Option<Blob>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extra: Option<u8>,
}

fn valid() -> ChallengeWire {
    ChallengeWire {
        protocol_version: Some(1),
        binding_mode: Some(3),
        security: Some(1),
        listener_id: Some(Blob::bin(16, 0x11)),
        handshake_id: Some(Blob::bin(32, 3)),
        salt: Some(Blob::bin(16, 7)),
        kdf: Some("argon2id".to_owned()),
        memory_kb: Some(19_456),
        time: Some(2),
        parallelism: Some(1),
        argon2_version: Some(0x13),
        server_nonce: Some(Blob::bin(32, 5)),
        extra: None,
    }
}

fn encode_challenge(wire: &ChallengeWire) -> Vec<u8> {
    rmp_serde::to_vec_named(wire).unwrap()
}

fn mutated(change: impl FnOnce(&mut ChallengeWire)) -> Vec<u8> {
    let mut wire = valid();
    change(&mut wire);
    encode_challenge(&wire)
}

fn assert_invalid_challenge(buf: &[u8]) {
    match decode_challenge(buf) {
        Err(ClientError::Protocol(msg)) => assert_eq!(msg, "invalid login challenge"),
        other => panic!("expected Protocol error, got {other:?}"),
    }
}

#[test]
fn valid_challenge_decodes() {
    let challenge = decode_challenge(&encode_challenge(&valid())).unwrap();
    assert_eq!(challenge.security, 1);
    assert_eq!(challenge.listener_id, [0x11u8; 16]);
    assert_eq!(challenge.handshake_id, [3u8; 32]);
    assert_eq!(challenge.salt, [7u8; 16]);
    assert_eq!(challenge.memory_kb, 19_456);
    assert_eq!(challenge.time, 2);
    assert_eq!(challenge.parallelism, 1);
    assert_eq!(challenge.argon2_version, 0x13);
    assert_eq!(challenge.server_nonce, [5u8; 32]);
}

#[test]
fn challenge_extra_key_is_rejected() {
    assert_invalid_challenge(&mutated(|w| w.extra = Some(1)));
}

#[test]
fn challenge_missing_key_is_rejected() {
    let removers: [fn(&mut ChallengeWire); 12] = [
        |w| w.protocol_version = None,
        |w| w.binding_mode = None,
        |w| w.security = None,
        |w| w.listener_id = None,
        |w| w.handshake_id = None,
        |w| w.salt = None,
        |w| w.kdf = None,
        |w| w.memory_kb = None,
        |w| w.time = None,
        |w| w.parallelism = None,
        |w| w.argon2_version = None,
        |w| w.server_nonce = None,
    ];
    for remove in removers {
        assert_invalid_challenge(&mutated(remove));
    }
}

#[test]
fn challenge_wrong_constants_are_rejected() {
    assert_invalid_challenge(&mutated(|w| w.kdf = Some("argon2i".to_owned())));
    assert_invalid_challenge(&mutated(|w| w.argon2_version = Some(0x10)));
    assert_invalid_challenge(&mutated(|w| w.binding_mode = Some(2)));
    assert_invalid_challenge(&mutated(|w| w.protocol_version = Some(2)));
}

#[test]
fn challenge_wrong_sizes_are_rejected() {
    assert_invalid_challenge(&mutated(|w| w.salt = Some(Blob::bin(15, 7))));
    assert_invalid_challenge(&mutated(|w| w.salt = Some(Blob::bin(17, 7))));
    assert_invalid_challenge(&mutated(|w| w.handshake_id = Some(Blob::bin(31, 3))));
    assert_invalid_challenge(&mutated(|w| w.server_nonce = Some(Blob::bin(33, 5))));
    assert_invalid_challenge(&mutated(|w| w.listener_id = Some(Blob::bin(15, 1))));
}

#[test]
fn challenge_binary_fields_must_be_bin() {
    assert_invalid_challenge(&mutated(|w| w.salt = Some(Blob::Str("s".repeat(16)))));
    assert_invalid_challenge(&mutated(|w| w.salt = Some(Blob::Arr(vec![7u8; 16]))));
}

#[test]
fn challenge_trailing_byte_is_rejected() {
    let mut buf = encode_challenge(&valid());
    buf.push(0);
    assert_invalid_challenge(&buf);
}

#[test]
fn challenge_non_map_input_is_rejected() {
    assert_invalid_challenge(&rmp_serde::to_vec(&valid()).unwrap());
    assert_invalid_challenge(&[]);
    assert_invalid_challenge(&[0xc1, 0xff, 0x00]);
}

/// Success map in which any key can be omitted or replaced.
#[derive(Serialize, Clone)]
struct FinishWire {
    #[serde(skip_serializing_if = "Option::is_none")]
    protocol_version: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    server_signature: Option<Blob>,
    #[serde(skip_serializing_if = "Option::is_none")]
    token: Option<Blob>,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Blob>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    channels: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    databases: Option<Option<Vec<String>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_at_ns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at_ns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extra: Option<u8>,
}

fn token_string() -> String {
    format!("shk1.{}.{}", TokenId([9u8; 16]).to_b64(), "A".repeat(43))
}

fn valid_finish() -> FinishWire {
    FinishWire {
        protocol_version: Some(1),
        server_signature: Some(Blob::bin(32, 0xab)),
        token: Some(Blob::Str(token_string())),
        id: Some(Blob::bin(16, 9)),
        label: Some("http-login".to_owned()),
        owner: Some("alice".to_owned()),
        mode: Some("read_write".to_owned()),
        channels: Some("both".to_owned()),
        databases: Some(Some(vec!["alpha".to_owned(), "zeta".to_owned()])),
        created_at_ns: Some(1_000),
        expires_at_ns: Some(2_000),
        extra: None,
    }
}

fn finish_mutated(change: impl FnOnce(&mut FinishWire)) -> Vec<u8> {
    let mut wire = valid_finish();
    change(&mut wire);
    rmp_serde::to_vec_named(&wire).unwrap()
}

fn assert_invalid_finish(buf: &[u8]) {
    match decode_finish(buf) {
        Err(ClientError::Protocol(msg)) => assert_eq!(msg, "invalid login response"),
        other => panic!("expected Protocol error, got {other:?}"),
    }
}

#[test]
fn valid_finish_decodes_with_databases() {
    let buf = rmp_serde::to_vec_named(&valid_finish()).unwrap();
    let success = decode_finish(&buf).unwrap();
    assert_eq!(success.server_signature, [0xabu8; 32]);
    assert_eq!(success.token.as_str(), token_string());
    assert_eq!(success.id, [9u8; 16]);
    assert_eq!(success.label, "http-login");
    assert_eq!(success.owner, "alice");
    assert_eq!(success.mode, LoginMode::ReadWrite);
    assert_eq!(success.channels, LoginChannels::Both);
    assert_eq!(
        success.databases,
        Some(vec!["alpha".to_owned(), "zeta".to_owned()])
    );
    assert_eq!(success.created_at_ns, 1_000);
    assert_eq!(success.expires_at_ns, 2_000);
}

#[test]
fn valid_finish_decodes_with_nil_databases() {
    let buf = finish_mutated(|w| w.databases = Some(None));
    let success = decode_finish(&buf).unwrap();
    assert_eq!(success.databases, None);
}

#[test]
fn finish_rejects_unknown_and_missing_keys() {
    assert_invalid_finish(&finish_mutated(|w| w.extra = Some(1)));
    assert_invalid_finish(&finish_mutated(|w| w.databases = None));
    assert_invalid_finish(&finish_mutated(|w| w.protocol_version = None));
    assert_invalid_finish(&finish_mutated(|w| w.expires_at_ns = None));
}

#[test]
fn finish_rejects_bad_field_shapes() {
    assert_invalid_finish(&finish_mutated(|w| {
        w.token = Some(Blob::bin(71, b'A'));
    }));
    assert_invalid_finish(&finish_mutated(|w| w.id = Some(Blob::bin(15, 9))));
    assert_invalid_finish(&finish_mutated(|w| w.id = Some(Blob::bin(17, 9))));
    assert_invalid_finish(&finish_mutated(|w| {
        w.server_signature = Some(Blob::bin(31, 1));
    }));
    assert_invalid_finish(&finish_mutated(|w| w.mode = Some("bogus".to_owned())));
}

#[test]
fn finish_rejects_trailing_byte_and_garbage() {
    let mut buf = rmp_serde::to_vec_named(&valid_finish()).unwrap();
    buf.push(0);
    assert_invalid_finish(&buf);
    assert_invalid_finish(&[]);
    assert_invalid_finish(&[0xc1]);
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BeginStrict {
    protocol_version: u8,
    user: String,
    client_nonce: ByteBuf,
    binding_mode: u8,
    token: TokenStrict,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenStrict {
    label: String,
    mode: String,
    channels: String,
    databases: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FinishStrict {
    protocol_version: u8,
    handshake_id: ByteBuf,
    client_proof: ByteBuf,
}

fn contains(buf: &[u8], needle: &[u8]) -> bool {
    buf.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn encode_begin_has_exactly_the_spec_keys() {
    let nonce = [6u8; 32];
    let buf = encode_begin("alice", &nonce, &LoginScope::default()).unwrap();
    assert_eq!(buf[0], 0x85);
    assert!(contains(&buf, b"\xacclient_nonce\xc4\x20"));
    assert!(contains(&buf, b"\xa5token\x84"));
    assert!(contains(&buf, b"\xa9databases\xc0"));
    let begin: BeginStrict = rmp_serde::from_slice(&buf).unwrap();
    assert_eq!(begin.protocol_version, 1);
    assert_eq!(begin.user, "alice");
    assert_eq!(begin.client_nonce.as_slice(), nonce.as_slice());
    assert_eq!(begin.binding_mode, 3);
    assert_eq!(begin.token.label, "http-login");
    assert_eq!(begin.token.mode, "read_only");
    assert_eq!(begin.token.channels, "rest");
    assert_eq!(begin.token.databases, None);
}

#[test]
fn encode_begin_carries_scope() {
    let nonce = [6u8; 32];
    let scope = LoginScope::default()
        .with_label("ci")
        .with_mode(LoginMode::ReadWrite)
        .with_channels(LoginChannels::Ws)
        .with_databases(Some(vec!["alpha".to_owned()]));
    let buf = encode_begin("alice", &nonce, &scope).unwrap();
    let begin: BeginStrict = rmp_serde::from_slice(&buf).unwrap();
    assert_eq!(begin.token.label, "ci");
    assert_eq!(begin.token.mode, "read_write");
    assert_eq!(begin.token.channels, "ws");
    assert_eq!(begin.token.databases, Some(vec!["alpha".to_owned()]));
}

#[test]
fn encode_finish_has_exactly_the_spec_keys() {
    let buf = encode_finish(&[3u8; 32], &[4u8; 32]).unwrap();
    assert_eq!(buf[0], 0x83);
    assert!(contains(&buf, b"\xachandshake_id\xc4\x20"));
    assert!(contains(&buf, b"\xacclient_proof\xc4\x20"));
    let finish: FinishStrict = rmp_serde::from_slice(&buf).unwrap();
    assert_eq!(finish.protocol_version, 1);
    assert_eq!(finish.handshake_id.as_slice(), [3u8; 32].as_slice());
    assert_eq!(finish.client_proof.as_slice(), [4u8; 32].as_slice());
}
