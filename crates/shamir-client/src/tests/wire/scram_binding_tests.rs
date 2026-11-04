//! Drives `run_scram` over an in-memory duplex pipe against an emulated SCRAM
//! server for both channel-binding shapes (TCP + TLS exporter, Unix + none).

use std::time::Duration;

use crate::client::transport::{WireAuthInit, WireAuthOk, WireChallenge, WireClientProof};
use crate::client::{run_scram, ScramRequest};
use shamir_connect::common::crypto::Ed25519Keypair;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::scram::{build_client_proof, DerivedKeys};
use shamir_connect::common::types::{BindingMode, ProtocolVersion, TransportKind};
use shamir_connect::common::username::NormalizedUsername;
use shamir_connect::server::{
    AuthInitView, ListenerPolicy, ProofOutcome, ServerHandshake, ServerSecrets, UserRecord,
};
use tokio::io::split;

const PASSWORD: &[u8] = b"correct horse battery staple";
const SALT: [u8; 16] = [5; 16];
const KDF: KdfParams = KdfParams {
    memory_kb: 19_456,
    time: 2,
    parallelism: 1,
    argon2_version: 0x13,
};
const SESSION_MAX_AGE_NS: u64 = 60_000_000_000;
const SERVER_QUERY_VERSION: u8 = 2;

async fn read_frame<R: tokio::io::AsyncRead + Unpin>(reader: &mut R) -> Vec<u8> {
    shamir_transport_tcp::framing::read_frame(reader, 64 * 1024)
        .await
        .expect("read frame")
}

async fn write_frame<W: tokio::io::AsyncWrite + Unpin>(writer: &mut W, bytes: &[u8]) {
    shamir_transport_tcp::framing::write_frame(writer, bytes)
        .await
        .expect("write frame");
}

/// Emulated SCRAM server; returns the session id it issued.
async fn run_server<R, W>(
    reader: &mut R,
    writer: &mut W,
    transport_kind: TransportKind,
    binding_mode: BindingMode,
    exporter: [u8; 32],
) -> [u8; 32]
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let init: WireAuthInit =
        rmp_serde::from_slice(&read_frame(reader).await).expect("typed auth_init");
    assert_eq!(init.binding_mode, binding_mode as u8);
    assert_eq!(init.version, ProtocolVersion::V1.as_u8());
    let username = NormalizedUsername::from_raw(&init.user).expect("normalize username");
    let client_nonce: [u8; 32] = init
        .client_nonce
        .as_slice()
        .try_into()
        .expect("client nonce width");
    let salt = SALT;
    let keys = DerivedKeys::derive(PASSWORD, &salt, &KDF).expect("derive SCRAM keys");
    let record = UserRecord {
        salt,
        stored_key: keys.stored_key,
        server_key: keys.server_key,
        kdf_params: KDF,
        tickets_invalid_before_ns: 0,
    };
    let secrets = ServerSecrets {
        server_secret: [1; 32],
        lockout_secret: [2; 32],
    };
    let handshake = ServerHandshake::new(
        ListenerPolicy::new(binding_mode),
        transport_kind,
        &secrets,
        AuthInitView {
            user: username.clone(),
            client_nonce,
            binding_mode,
            version: init.version,
        },
        exporter,
        KDF,
        |_| Some(record),
    )
    .expect("construct ServerHandshake");
    let challenge = handshake.challenge();
    let challenge_wire = WireChallenge {
        salt: challenge.salt.to_vec(),
        memory_kb: challenge.kdf_params.memory_kb,
        time: challenge.kdf_params.time,
        parallelism: challenge.kdf_params.parallelism,
        argon2_version: challenge.kdf_params.argon2_version,
        server_nonce: challenge.server_nonce.to_vec(),
    };
    write_frame(
        writer,
        &rmp_serde::to_vec(&challenge_wire).expect("serialize challenge"),
    )
    .await;

    let auth_message = shamir_connect::common::AuthMessage::build(
        shamir_connect::common::auth_message::AuthMessageInputs {
            username: &username,
            client_nonce: &client_nonce,
            server_nonce: &challenge.server_nonce,
            salt: &challenge.salt,
            kdf_params: challenge.kdf_params,
            transport_kind,
            binding_mode,
            tls_exporter_or_zeros: &exporter,
            supported_version: ProtocolVersion::V1,
        },
    )
    .expect("build auth message");
    let client_keys = DerivedKeys::derive(PASSWORD, &challenge.salt, &challenge.kdf_params)
        .expect("derive client keys");
    let expected_proof = build_client_proof(
        &client_keys.client_key,
        &client_keys.stored_key,
        &auth_message,
    );
    let wire_proof: WireClientProof =
        rmp_serde::from_slice(&read_frame(reader).await).expect("typed client proof");
    let proof: [u8; 32] = wire_proof
        .client_proof
        .as_slice()
        .try_into()
        .expect("proof width");
    assert_eq!(proof, expected_proof);
    let ProofOutcome::Accepted(auth_ok) = handshake
        .verify_proof(
            &proof,
            &Ed25519Keypair::from_seed(&[9; 32]),
            SESSION_MAX_AGE_NS,
        )
        .expect("verify SCRAM")
    else {
        panic!("valid proof rejected");
    };
    let typed = WireAuthOk {
        server_signature: auth_ok.server_signature.to_vec(),
        server_pub_key: auth_ok.server_pub_key.to_vec(),
        identity_sig: auth_ok.identity_sig.to_vec(),
        session_id: auth_ok.session_id.to_vec(),
        expires_at_ns: auth_ok.expires_at_ns,
        resumption_ticket: Vec::new(),
        resumption_expires_at_ns: 0,
        server_query_version: SERVER_QUERY_VERSION,
        rotation_in_progress: None,
        kdf_upgrade_required: None,
    };
    write_frame(
        writer,
        &rmp_serde::to_vec(&typed).expect("serialize auth_ok"),
    )
    .await;
    auth_ok.session_id
}

async fn run_binding(transport_kind: TransportKind, binding_mode: BindingMode, exporter: [u8; 32]) {
    let (client_end, server_end) = tokio::io::duplex(64 * 1024);
    let (mut client_r, mut client_w) = split(client_end);
    let (mut server_r, mut server_w) = split(server_end);

    let server = run_server(
        &mut server_r,
        &mut server_w,
        transport_kind,
        binding_mode,
        exporter,
    );
    let client = run_scram(
        &mut client_r,
        &mut client_w,
        ScramRequest {
            username: NormalizedUsername::from_raw("scram-binding").unwrap(),
            transport_kind,
            binding_mode,
            tls_exporter: exporter,
            trusted_pin: None,
            accept_new_host: true,
            password: PASSWORD,
            phase: None,
        },
    );

    let (session_id, outcome) = tokio::time::timeout(Duration::from_secs(60), async {
        tokio::join!(server, client)
    })
    .await
    .expect("SCRAM exchange timed out");
    let outcome = outcome.expect("run_scram succeeds");
    assert_eq!(outcome.verified.success.session_id, session_id);
    assert_eq!(outcome.server_query_version, SERVER_QUERY_VERSION);
}

#[tokio::test]
async fn scram_completes_with_tcp_tls_exporter_binding() {
    run_binding(TransportKind::Tcp, BindingMode::TlsExporter, [0x5A; 32]).await;
}

#[tokio::test]
async fn scram_completes_with_unix_none_binding() {
    run_binding(TransportKind::Unix, BindingMode::None, [0u8; 32]).await;
}
