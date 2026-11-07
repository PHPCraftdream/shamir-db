use crate::server::http_login::{
    build_transcript, encode_scope, HttpLoginChannels, HttpLoginMode, HttpLoginScope,
    HttpLoginTranscriptInput,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct FixtureFile {
    cases: Vec<Fixture>,
}

#[derive(Deserialize)]
struct Fixture {
    input: Input,
    scope_bytes_hex: String,
    transcript_hex: String,
}

#[derive(Deserialize)]
struct Input {
    protocol_version: u8,
    binding_mode: u8,
    security: u8,
    listener_id_hex: String,
    handshake_id_hex: String,
    user: String,
    client_nonce_hex: String,
    server_nonce_hex: String,
    salt_hex: String,
    memory_kb: u32,
    time: u32,
    parallelism: u32,
    argon2_version: u8,
    scope: Scope,
}

#[derive(Deserialize)]
struct Scope {
    label: String,
    mode: String,
    channels: String,
    databases: Option<Vec<String>>,
}

fn array<const N: usize>(hex: &str) -> [u8; N] {
    hex::decode(hex)
        .expect("fixture hex")
        .try_into()
        .expect("fixture field width")
}

#[test]
fn hand_derived_transcript_vectors_and_length_formula() {
    let fixture: FixtureFile = toml::from_str(include_str!(
        "../../../../../test-vectors/http_login_transcript_v1.toml"
    ))
    .expect("valid TOML vectors");
    assert_eq!(fixture.cases.len(), 3);
    for case in fixture.cases {
        let mode = match case.input.scope.mode.as_str() {
            "read_only" => HttpLoginMode::ReadOnly,
            "read_write" => HttpLoginMode::ReadWrite,
            _ => panic!("fixture mode"),
        };
        let channels = match case.input.scope.channels.as_str() {
            "rest" => HttpLoginChannels::Rest,
            "ws" => HttpLoginChannels::Ws,
            "both" => HttpLoginChannels::Both,
            _ => panic!("fixture channels"),
        };
        let database_refs = case
            .input
            .scope
            .databases
            .as_ref()
            .map(|items| items.iter().map(String::as_str).collect::<Vec<_>>());
        let scope = HttpLoginScope {
            label: &case.input.scope.label,
            mode,
            channels,
            databases: database_refs.as_deref(),
        };
        let scope_bytes = encode_scope(&scope).expect("scope");
        assert_eq!(
            scope_bytes,
            hex::decode(&case.scope_bytes_hex).expect("scope vector hex")
        );
        let listener = array::<16>(&case.input.listener_id_hex);
        let handshake = array::<32>(&case.input.handshake_id_hex);
        let client_nonce = array::<32>(&case.input.client_nonce_hex);
        let server_nonce = array::<32>(&case.input.server_nonce_hex);
        let salt = array::<16>(&case.input.salt_hex);
        let input = HttpLoginTranscriptInput {
            protocol_version: case.input.protocol_version,
            binding_mode: case.input.binding_mode,
            security: case.input.security,
            listener_id: &listener,
            handshake_id: &handshake,
            user: &case.input.user,
            client_nonce: &client_nonce,
            server_nonce: &server_nonce,
            salt: &salt,
            memory_kb: case.input.memory_kb,
            time: case.input.time,
            parallelism: case.input.parallelism,
            argon2_version: case.input.argon2_version,
            scope_bytes: &scope_bytes,
        };
        let transcript = build_transcript(&input).expect("transcript");
        assert_eq!(
            transcript,
            hex::decode(&case.transcript_hex).expect("transcript vector hex")
        );
        assert_eq!(
            transcript.len(),
            166 + case.input.user.len() + scope_bytes.len()
        );
    }
}
