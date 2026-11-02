use crate::server::http_login::{
    build_server_signature, build_transcript, encode_scope, verify_http_login_proof,
    HttpLoginChannels, HttpLoginMode, HttpLoginScope, HttpLoginTranscriptInput,
};
use crate::common::crypto::{hmac_sha256, sha256, StoredKey};
use crate::common::scram::ClientProof;
use crate::server::lockout::{InMemoryLockoutStore, PairKey, PairStatus, Subnet};
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;
use zeroize::Zeroizing;

#[derive(Deserialize)]
struct FixtureFile { cases: Vec<Fixture> }
#[derive(Deserialize)]
struct Fixture { input: Input, scope_bytes_hex: String, transcript_hex: String }
#[derive(Deserialize)]
struct Input {
    protocol_version: u8, binding_mode: u8, security: u8,
    listener_id_hex: String, handshake_id_hex: String, user: String,
    client_nonce_hex: String, server_nonce_hex: String, salt_hex: String,
    memory_kb: u32, time: u32, parallelism: u32, argon2_version: u8,
    scope: Scope,
}
#[derive(Deserialize)]
struct Scope { label: String, mode: String, channels: String, databases: Option<Vec<String>> }

fn decode_hex(source: &str) -> Vec<u8> {
    source.as_bytes().chunks_exact(2).map(|chunk| {
        let text = std::str::from_utf8(chunk).expect("fixture hex is ASCII");
        u8::from_str_radix(text, 16).expect("fixture hex digits")
    }).collect()
}
fn array<const N: usize>(hex: &str) -> [u8; N] {
    decode_hex(hex).try_into().expect("fixture field width")
}

#[test]
fn hand_derived_transcript_vectors_and_length_formula() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-vectors/http_login_transcript_v1.json");
    let source = fs::read(path).expect("vector fixture");
    let fixture: FixtureFile = serde_json::from_slice(&source).expect("valid vector json");
    assert_eq!(fixture.cases.len(), 2);
    for case in fixture.cases {
        let mode = match case.input.scope.mode.as_str() { "read_only" => HttpLoginMode::ReadOnly, "read_write" => HttpLoginMode::ReadWrite, _ => panic!("fixture mode") };
        let channels = match case.input.scope.channels.as_str() { "rest" => HttpLoginChannels::Rest, "ws" => HttpLoginChannels::Ws, "both" => HttpLoginChannels::Both, _ => panic!("fixture channels") };
        let database_refs = case.input.scope.databases.as_ref().map(|items| items.iter().map(String::as_str).collect::<Vec<_>>());
        let scope = HttpLoginScope { label: &case.input.scope.label, mode, channels, databases: database_refs.as_deref() };
        let scope_bytes = encode_scope(&scope).expect("scope");
        assert_eq!(scope_bytes, decode_hex(&case.scope_bytes_hex));
        let listener = array::<16>(&case.input.listener_id_hex);
        let handshake = array::<32>(&case.input.handshake_id_hex);
        let client_nonce = array::<32>(&case.input.client_nonce_hex);
        let server_nonce = array::<32>(&case.input.server_nonce_hex);
        let salt = array::<16>(&case.input.salt_hex);
        let input = HttpLoginTranscriptInput {
            protocol_version: case.input.protocol_version, binding_mode: case.input.binding_mode,
            security: case.input.security, listener_id: &listener, handshake_id: &handshake,
            user: &case.input.user, client_nonce: &client_nonce, server_nonce: &server_nonce,
            salt: &salt, memory_kb: case.input.memory_kb, time: case.input.time,
            parallelism: case.input.parallelism, argon2_version: case.input.argon2_version,
            scope_bytes: &scope_bytes,
        };
        let transcript = build_transcript(&input).expect("transcript");
        assert_eq!(transcript, decode_hex(&case.transcript_hex));
        assert_eq!(transcript.len(), 166 + case.input.user.len() + scope_bytes.len());
    }
}

fn scope(label: &str, databases: Option<&[&str]>) -> HttpLoginScope<'_> {
    HttpLoginScope { label, mode: HttpLoginMode::ReadOnly, channels: HttpLoginChannels::Rest, databases }
}

#[test]
fn scope_rejects_control_bytes_and_invalid_database_lists() {
    let label = "x".repeat(129);
    for label in ["", "x\u{1f}", "x\u{7f}", label.as_str()] {
        assert!(encode_scope(&scope(label, None)).is_err(), "label {label:?}");
    }
    let long_name = "d".repeat(256);
    let left = ["a"];
    assert!(encode_scope(&scope("x", Some(&left))).is_ok());
    let max_label = "x".repeat(128);
    assert!(encode_scope(&scope(max_label.as_str(), None)).is_ok());
    let exactly_max = "z".repeat(255);
    let encoded_max = encode_scope(&scope("x", Some(&[exactly_max.as_str()]))).expect("maximum database bytes");
    assert_eq!(encoded_max.len(), 263);
    let empty_db: &[&str] = &[];
    assert!(encode_scope(&scope("x", Some(empty_db))).is_err());
    let empty: &[&str] = &[];
    let duplicate = ["a", "a"];
    let reverse = ["b", "a"];
    let punctuated = ["bad.name"];
    let too_long = [long_name.as_str()];
    let invalid_lists: [&[&str]; 5] = [empty, &duplicate, &reverse, &punctuated, &too_long];
    for databases in invalid_lists {
        assert!(encode_scope(&scope("x", Some(databases))).is_err());
    }
    let invalid_name = ["é"];
    assert!(encode_scope(&scope("x", Some(&invalid_name))).is_err());
    let too_many = ["a", "b", "c", "d", "e"];
    assert!(encode_scope(&scope("x", Some(&too_many))).is_err());
    let four_names = ["a", "b", "c", "d"];
    assert!(encode_scope(&scope("x", Some(&four_names))).is_ok());
    let exactly_max_label = "x".repeat(128);
    let one_db = "z".repeat(255);
    let max_scope = encode_scope(&scope(&exactly_max_label, Some(&[one_db.as_str()]))).expect("scope max");
    assert!(max_scope.len() <= 512);
    let too_large_pair = ["a".repeat(255), "b".repeat(255)];
    assert!(encode_scope(&scope("x", Some(&[too_large_pair[0].as_str(), too_large_pair[1].as_str()]))).is_err());
}

#[test]
fn scope_total_encoded_size_preflight_rejects_oversized_combination() {
    let label = "x".repeat(128);
    let db_one = "a".repeat(255);
    let db_two = "b".repeat(255);
    assert!(encode_scope(&scope(&label, Some(&[db_one.as_str(), db_two.as_str()]))).is_err());
    let max_label = encode_scope(&scope(&label, None)).expect("maximum label");
    assert_eq!(max_label.len(), 133);
    assert!(encode_scope(&scope("x\u{00}", None)).is_err());
}
#[test]
fn transcript_rejects_invalid_metadata_kdf_username_nonces_and_scope_shapes() {
    let listener = [0; 16];
    let handshake = [3; 32];
    let client_nonce = [1; 32];
    let server_nonce = [2; 32];
    let salt = [4; 16];
    let valid_scope = [0, 1, b'x', 0, 0, 0xff];
    let mut input = HttpLoginTranscriptInput {
        protocol_version: 1, binding_mode: 3, security: 0,
        listener_id: &listener, handshake_id: &handshake, user: "a",
        client_nonce: &client_nonce, server_nonce: &server_nonce, salt: &salt,
        memory_kb: 19_456, time: 2, parallelism: 1, argon2_version: 0x13,
        scope_bytes: &valid_scope,
    };
    assert!(build_transcript(&input).is_ok());
    input.protocol_version = 2; assert!(build_transcript(&input).is_err());
    input.protocol_version = 1;
    input.protocol_version = 0; assert!(build_transcript(&input).is_err());
    input.protocol_version = 1;
    input.binding_mode = 2; assert!(build_transcript(&input).is_err());
    input.binding_mode = 3;
    input.security = 3; assert!(build_transcript(&input).is_err());
    input.security = 1; assert!(build_transcript(&input).is_ok());
    input.security = 2; assert!(build_transcript(&input).is_ok());
    input.memory_kb = 19_455; assert!(build_transcript(&input).is_err());
    input.memory_kb = 19_456; assert!(build_transcript(&input).is_ok());
    input.memory_kb = 19_457; assert!(build_transcript(&input).is_ok());
    input.memory_kb = 262_144; assert!(build_transcript(&input).is_ok());
    input.memory_kb = 262_145; assert!(build_transcript(&input).is_err());
    input.time = 1; assert!(build_transcript(&input).is_err());
    input.time = 2; assert!(build_transcript(&input).is_ok());
    input.time = 8; assert!(build_transcript(&input).is_ok());
    input.time = 9; assert!(build_transcript(&input).is_err());
    input.parallelism = 0; assert!(build_transcript(&input).is_err());
    input.parallelism = 1; assert!(build_transcript(&input).is_ok());
    input.parallelism = 8; assert!(build_transcript(&input).is_ok());
    input.parallelism = 9; assert!(build_transcript(&input).is_err());
    input.parallelism = 1;
    let long_user = "a".repeat(256); input.user = &long_user; assert!(build_transcript(&input).is_err());
    input.user = "a";
    input.argon2_version = 0x12; assert!(build_transcript(&input).is_err());
    input.argon2_version = 0x13; assert!(build_transcript(&input).is_ok());
    input.user = "A"; assert!(build_transcript(&input).is_err());
    input.user = "a\u{301}"; assert!(build_transcript(&input).is_err());
    input.user = "a";
    let zero = [0; 32]; input.client_nonce = &zero; assert!(build_transcript(&input).is_err()); input.client_nonce = &client_nonce;
    input.server_nonce = &client_nonce; assert!(build_transcript(&input).is_err()); input.server_nonce = &server_nonce;
    input.server_nonce = &zero; assert!(build_transcript(&input).is_err()); input.server_nonce = &server_nonce;
    input.server_nonce = &client_nonce; assert!(build_transcript(&input).is_err()); input.server_nonce = &server_nonce;
    input.scope_bytes = &[0, 1, b'x', 0, 0, 1, 0, 1, 0xff];
    assert!(build_transcript(&input).is_err());
    input.scope_bytes = &valid_scope;
    for invalid in [
        &[0, 1, b'x', 2, 0, 0xff][..],
        &[0, 1, b'x', 0, 0, 0][..],
        &[0, 1, b'x', 0, 0, 0xff, 1][..],
        &[0, 1, b'x', 0, 0, 1, 0, 2, b'b', b'a'][..],
        &[0, 1, b'x', 0, 0, 1, 0, 1, b'a', 0][..],
        &[0, 1, b'x', 0, 0, 1, 0, 0][..],
        &[0, 1, b'x', 0, 0, 2, 0, 1, b'a'][..],
    ] {
        input.scope_bytes = invalid;
        assert!(build_transcript(&input).is_err());
    }
}

#[test]
fn proof_round_trip_and_rejects_tamper_wrong_key_and_truncation() {
    let transcript = b"http-login transcript fixture";
    let client_key = Zeroizing::new([0x31; 32]);
    let stored = StoredKey(sha256(&client_key[..]));
    let signature = hmac_sha256(&stored.0, transcript);
    let proof: ClientProof = core::array::from_fn(|i| client_key[i] ^ signature[i]);
    assert!(verify_http_login_proof(&proof, &stored, transcript));
    let mut changed_proof = proof;
    changed_proof[0] ^= 1;
    assert!(!verify_http_login_proof(&changed_proof, &stored, transcript));
    assert!(!verify_http_login_proof(&proof, &StoredKey([0x99; 32]), transcript));
    assert!(!verify_http_login_proof(&proof[..31].try_into().expect("fixed proof"), &stored, transcript));
    assert!(!verify_http_login_proof(&proof, &stored, &transcript[..transcript.len() - 1]));
    assert!(!verify_http_login_proof(&proof, &stored, b""));
    let server_key = [0x42; 32];
    let server_signature = build_server_signature(&server_key, transcript);
    assert_eq!(server_signature, hmac_sha256(&server_key, transcript));
    let altered_transcript = b"http-login transcript Fixture";
    assert_ne!(server_signature, build_server_signature(&server_key, altered_transcript));
}
#[test]
fn transcript_rejects_oversized_username_and_zero_length_scope() {
    let listener = [0; 16];
    let handshake = [3; 32];
    let client_nonce = [1; 32];
    let server_nonce = [2; 32];
    let salt = [4; 16];
    let scope_bytes = [0, 1, b'x', 0, 0, 0xff];
    let long_user = "a".repeat(256);
    let mut input = HttpLoginTranscriptInput {
        protocol_version: 1, binding_mode: 3, security: 0,
        listener_id: &listener, handshake_id: &handshake, user: &long_user,
        client_nonce: &client_nonce, server_nonce: &server_nonce, salt: &salt,
        memory_kb: 19_456, time: 2, parallelism: 1, argon2_version: 0x13,
        scope_bytes: &scope_bytes,
    };
    assert!(build_transcript(&input).is_err());
    input.user = "a";
    input.scope_bytes = &[0, 0, 0];
    assert!(build_transcript(&input).is_err());
}
#[test]
fn pair_status_reports_fresh_backoff_lock_and_stale_pair_without_mutation() {
    let store = InMemoryLockoutStore::new();
    let pair: PairKey = (Subnet::V4([10, 1, 2]), [9; 16]);
    let fresh = PairStatus { locked_until_ns: None, backoff_until_ns: None, last_fail_at_ns: None };
    assert_eq!(store.pair_status(pair, 100), fresh);
    store.register_failure(pair, 1_000_000_000);
    let backoff = store.pair_status(pair, 1_000_000_000);
    assert_eq!(backoff.backoff_until_ns, Some(1_100_000_000));
    assert_eq!(backoff.last_fail_at_ns, Some(1_000_000_000));
    assert_eq!(store.pair_status(pair, 1_050_000_000), backoff);
    assert_eq!(store.pair_status(pair, 1_100_000_000).backoff_until_ns, None);
    for n in 1..50 { store.register_failure(pair, 2_000_000_000 + u64::from(n) * 1_000_000_000); }
    let locked = store.pair_status(pair, 60_000_000_000);
    assert_eq!(locked.locked_until_ns, Some(3_651_000_000_000));
    assert!(locked.backoff_until_ns.is_some());
    let stale = store.pair_status(pair, 3_652_000_000_001);
    assert_eq!(stale.locked_until_ns, None);
    assert_eq!(stale.backoff_until_ns, None);
    assert_eq!(stale.last_fail_at_ns, Some(51_000_000_000));
}
