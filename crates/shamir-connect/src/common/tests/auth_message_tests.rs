//! Tests for canonical `auth_message` byte layout (spec §4.1).
//!
//! The reference vector lives in `test-vectors/auth_message_default.toml`
//! and is the spec §16 inline example. Any change here breaks interop with
//! every other implementation (browser SDK, future Go/Python clients).

use crate::common::auth_message::{AuthMessage, AuthMessageInputs};
use crate::common::kdf_params::KdfParams;
use crate::common::types::{BindingMode, ProtocolVersion, TransportKind};
use crate::common::username::NormalizedUsername;

const TEST_VECTOR_TOML: &str = include_str!("../../../test-vectors/auth_message_default.toml");

#[derive(serde::Deserialize)]
struct Vector {
    inputs: VecInputs,
    expected: VecExpected,
}

#[derive(serde::Deserialize)]
struct VecInputs {
    username: String,
    client_nonce_hex: String,
    server_nonce_hex: String,
    salt_hex: String,
    kdf_params: VecKdfParams,
    transport_kind: u8,
    binding_mode: u8,
    tls_exporter_hex: String,
    supported_version: u8,
}

#[derive(serde::Deserialize)]
struct VecKdfParams {
    memory_kb: u32,
    time: u32,
    parallelism: u32,
    argon2_version: u8,
}

#[derive(serde::Deserialize)]
struct VecExpected {
    auth_message_total_bytes: usize,
    auth_message_hex: String,
}

fn hex_to_array<const N: usize>(s: &str) -> [u8; N] {
    let bytes = hex::decode(s).expect("valid hex");
    assert_eq!(
        bytes.len(),
        N,
        "hex length mismatch: got {}, expected {}",
        bytes.len(),
        N
    );
    let mut out = [0u8; N];
    out.copy_from_slice(&bytes);
    out
}

#[test]
fn matches_spec_section_16_inline_vector() {
    let v: Vector = toml::from_str(TEST_VECTOR_TOML).unwrap();

    let username = NormalizedUsername::from_raw(&v.inputs.username).unwrap();
    let client_nonce = hex_to_array::<32>(&v.inputs.client_nonce_hex);
    let server_nonce = hex_to_array::<32>(&v.inputs.server_nonce_hex);
    let salt = hex_to_array::<16>(&v.inputs.salt_hex);
    let tls_exporter = hex_to_array::<32>(&v.inputs.tls_exporter_hex);

    let kdf = KdfParams {
        memory_kb: v.inputs.kdf_params.memory_kb,
        time: v.inputs.kdf_params.time,
        parallelism: v.inputs.kdf_params.parallelism,
        argon2_version: v.inputs.kdf_params.argon2_version,
    };

    let inputs = AuthMessageInputs {
        username: &username,
        client_nonce: &client_nonce,
        server_nonce: &server_nonce,
        salt: &salt,
        kdf_params: kdf,
        transport_kind: TransportKind::from_u8(v.inputs.transport_kind).unwrap(),
        binding_mode: BindingMode::from_u8(v.inputs.binding_mode).unwrap(),
        tls_exporter_or_zeros: &tls_exporter,
        supported_version: ProtocolVersion(v.inputs.supported_version),
    };

    let am = AuthMessage::build(inputs).unwrap();

    assert_eq!(
        am.len(),
        v.expected.auth_message_total_bytes,
        "auth_message length mismatch"
    );

    let actual_hex = hex::encode(am.as_bytes());
    assert_eq!(
        actual_hex, v.expected.auth_message_hex,
        "auth_message bytes do not match spec §16 vector"
    );
}

#[test]
fn header_is_exactly_shamir_auth_v1() {
    let username = NormalizedUsername::from_raw("x").unwrap();
    let zero32 = [0u8; 32];
    let zero16 = [0u8; 16];
    let mut nonce = [0u8; 32];
    nonce[0] = 1; // non-zero

    let am = AuthMessage::build(AuthMessageInputs {
        username: &username,
        client_nonce: &nonce,
        server_nonce: &nonce,
        salt: &zero16,
        kdf_params: KdfParams::DEFAULT,
        transport_kind: TransportKind::Tcp,
        binding_mode: BindingMode::TlsExporter,
        tls_exporter_or_zeros: &zero32,
        supported_version: ProtocolVersion::V1,
    })
    .unwrap();

    // First 14 bytes are the header.
    assert_eq!(&am.as_bytes()[..14], b"SHAMIR-AUTH-v1");
}

#[test]
fn rejects_all_zero_client_nonce() {
    let username = NormalizedUsername::from_raw("x").unwrap();
    let zero32 = [0u8; 32];
    let zero16 = [0u8; 16];
    let mut nonce = [0u8; 32];
    nonce[0] = 1;

    let err = AuthMessage::build(AuthMessageInputs {
        username: &username,
        client_nonce: &zero32, // all-zero — must reject
        server_nonce: &nonce,
        salt: &zero16,
        kdf_params: KdfParams::DEFAULT,
        transport_kind: TransportKind::Tcp,
        binding_mode: BindingMode::TlsExporter,
        tls_exporter_or_zeros: &zero32,
        supported_version: ProtocolVersion::V1,
    })
    .unwrap_err();

    matches!(err, crate::common::Error::InvalidInput(_));
}

#[test]
fn rejects_all_zero_server_nonce() {
    let username = NormalizedUsername::from_raw("x").unwrap();
    let zero32 = [0u8; 32];
    let zero16 = [0u8; 16];
    let mut nonce = [0u8; 32];
    nonce[0] = 1;

    let err = AuthMessage::build(AuthMessageInputs {
        username: &username,
        client_nonce: &nonce,
        server_nonce: &zero32, // all-zero — must reject
        salt: &zero16,
        kdf_params: KdfParams::DEFAULT,
        transport_kind: TransportKind::Tcp,
        binding_mode: BindingMode::TlsExporter,
        tls_exporter_or_zeros: &zero32,
        supported_version: ProtocolVersion::V1,
    })
    .unwrap_err();

    matches!(err, crate::common::Error::InvalidInput(_));
}

#[test]
fn length_matches_formula_for_default_inputs() {
    let username = NormalizedUsername::from_raw("alice").unwrap();
    let mut nonce = [0u8; 32];
    nonce[0] = 1;
    let zero32 = [0u8; 32];
    let zero16 = [0u8; 16];

    let am = AuthMessage::build(AuthMessageInputs {
        username: &username,
        client_nonce: &nonce,
        server_nonce: &nonce,
        salt: &zero16,
        kdf_params: KdfParams::DEFAULT,
        transport_kind: TransportKind::Tcp,
        binding_mode: BindingMode::TlsExporter,
        tls_exporter_or_zeros: &zero32,
        supported_version: ProtocolVersion::V1,
    })
    .unwrap();

    // Per spec §16: 14 + 2 + 5 + 32 + 32 + 16 + 4+4+4+1 + 1+1+32 + 1 = 149
    assert_eq!(am.len(), 149);
}
