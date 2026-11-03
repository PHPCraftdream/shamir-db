//! Additional boundary regressions retained from the original HTTP-login tests.

use crate::server::http_login::{
    build_transcript, encode_scope, HttpLoginChannels, HttpLoginMode, HttpLoginScope,
    HttpLoginTranscriptInput,
};

#[test]
fn transcript_accepts_minimum_and_maximum_wire_lengths() {
    let listener = [1; 16];
    let handshake = [2; 32];
    let client_nonce = [3; 32];
    let server_nonce = [4; 32];
    let salt = [5; 16];
    let min_scope = [0, 1, b'x', 0, 0, 0xff];
    let min_input = HttpLoginTranscriptInput {
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
        scope_bytes: &min_scope,
    };
    assert_eq!(build_transcript(&min_input).expect("minimum T").len(), 173);

    let label = "l".repeat(128);
    let database_names = [
        "a".repeat(92),
        "b".repeat(93),
        "c".repeat(93),
        "d".repeat(93),
    ];
    let database_refs = database_names
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let scope = encode_scope(&HttpLoginScope {
        label: &label,
        mode: HttpLoginMode::ReadOnly,
        channels: HttpLoginChannels::Rest,
        databases: Some(&database_refs),
    })
    .expect("maximum scope");
    assert_eq!(scope.len(), 512);
    let user = "a".repeat(255);
    let max_input = HttpLoginTranscriptInput {
        protocol_version: 1,
        binding_mode: 3,
        security: 2,
        listener_id: &listener,
        handshake_id: &handshake,
        user: &user,
        client_nonce: &client_nonce,
        server_nonce: &server_nonce,
        salt: &salt,
        memory_kb: 262_144,
        time: 8,
        parallelism: 8,
        argon2_version: 0x13,
        scope_bytes: &scope,
    };
    assert_eq!(build_transcript(&max_input).expect("maximum T").len(), 933);
}

#[test]
fn transcript_accepts_uppercase_ascii_database_names() {
    let listener = [1; 16];
    let handshake = [2; 32];
    let client_nonce = [3; 32];
    let server_nonce = [4; 32];
    let salt = [5; 16];
    let databases = ["A-b_9", "Main"];
    let scope = encode_scope(&HttpLoginScope {
        label: "x",
        mode: HttpLoginMode::ReadOnly,
        channels: HttpLoginChannels::Rest,
        databases: Some(&databases),
    })
    .expect("uppercase ASCII database syntax");
    let input = HttpLoginTranscriptInput {
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
        scope_bytes: &scope,
    };
    assert!(build_transcript(&input).is_ok());
}
