use crate::server::http_login::{build_transcript, HttpLoginTranscriptInput};

#[test]
fn transcript_rejects_invalid_metadata_kdf_username_nonces_and_scope_shapes() {
    let listener = [0; 16];
    let handshake = [3; 32];
    let client_nonce = [1; 32];
    let server_nonce = [2; 32];
    let salt = [4; 16];
    let valid_scope = [0, 1, b'x', 0, 0, 0xff];
    let mut input = HttpLoginTranscriptInput {
        protocol_version: 1,
        binding_mode: 3,
        security: 0,
        listener_id: &listener,
        handshake_id: &handshake,
        user: "a",
        client_nonce: &client_nonce,
        server_nonce: &server_nonce,
        salt: &salt,
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
        scope_bytes: &valid_scope,
    };
    assert!(build_transcript(&input).is_ok());
    input.protocol_version = 2;
    assert!(build_transcript(&input).is_err());
    input.protocol_version = 1;
    input.binding_mode = 2;
    assert!(build_transcript(&input).is_err());
    input.binding_mode = 3;
    input.security = 3;
    assert!(build_transcript(&input).is_err());
    input.security = 1;
    assert!(build_transcript(&input).is_ok());
    input.security = 2;
    assert!(build_transcript(&input).is_ok());
    input.memory_kb = 19_455;
    assert!(build_transcript(&input).is_err());
    input.memory_kb = 19_456;
    assert!(build_transcript(&input).is_ok());
    input.memory_kb = 262_145;
    assert!(build_transcript(&input).is_err());
    input.memory_kb = 262_144;
    assert!(build_transcript(&input).is_ok());
    input.memory_kb = 19_456;
    input.time = 1;
    assert!(build_transcript(&input).is_err());
    input.time = 9;
    assert!(build_transcript(&input).is_err());
    input.time = 8;
    assert!(build_transcript(&input).is_ok());
    input.time = 2;
    input.parallelism = 0;
    assert!(build_transcript(&input).is_err());
    input.parallelism = 9;
    assert!(build_transcript(&input).is_err());
    input.parallelism = 8;
    assert!(build_transcript(&input).is_ok());
    input.parallelism = 1;
    let long_user = "a".repeat(256);
    input.user = &long_user;
    assert!(build_transcript(&input).is_err());
    input.user = "Alice";
    assert!(build_transcript(&input).is_err());
    input.user = "A";
    assert!(build_transcript(&input).is_err());
    input.user = "a\u{301}";
    assert!(build_transcript(&input).is_err());
    input.user = "a";
    input.argon2_version = 0x12;
    assert!(build_transcript(&input).is_err());
    input.argon2_version = 0x13;
    let zero = [0; 32];
    input.client_nonce = &zero;
    assert!(build_transcript(&input).is_err());
    input.client_nonce = &client_nonce;
    input.server_nonce = &zero;
    assert!(build_transcript(&input).is_err());
    input.server_nonce = &client_nonce;
    assert!(build_transcript(&input).is_err());
    input.server_nonce = &server_nonce;
    for invalid in [
        &[0, 1, b'x', 2, 0, 0xff][..],
        &[0, 1, b'x', 0, 0, 0][..],
        &[0, 1, b'x', 0, 0, 0xff, 1][..],
        &[0, 1, b'x', 0, 0, 2, 0, 1, b'b', 0, 1, b'a'][..],
        &[0, 1, b'x', 0, 0, 1, 0, 1, b'a', 0][..],
        &[0, 1, b'x', 0, 0, 1, 0, 0][..],
        &[0, 1, b'x', 0, 0, 2, 0, 1, b'a'][..],
        &[0, 1, 0xff, 0, 0, 0xff][..],
    ] {
        input.scope_bytes = invalid;
        assert!(build_transcript(&input).is_err());
    }
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
        protocol_version: 1,
        binding_mode: 3,
        security: 0,
        listener_id: &listener,
        handshake_id: &handshake,
        user: &long_user,
        client_nonce: &client_nonce,
        server_nonce: &server_nonce,
        salt: &salt,
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
        scope_bytes: &scope_bytes,
    };
    assert!(build_transcript(&input).is_err());
    input.user = "a";
    input.scope_bytes = &[0, 0, 0];
    assert!(build_transcript(&input).is_err());
}
