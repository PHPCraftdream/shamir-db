//! Session-resume fast-path over the local-IPC transport (Unix domain
//! socket / Windows Named Pipe). Mirrors `resume_fast_path.rs`'s TLS+TCP
//! test exactly, minus TLS: full SCRAM on a first connection → capture the
//! issued ticket → open a SECOND, independent IPC connection → resume with
//! that ticket → server responds with `ResumeOkWire`.
//!
//! Regression coverage for TRANSPORT_UNIX.md §6 / §10.4's claim that ticket
//! issuance + resume works over `unix` unmodified — resume itself is wire-
//! protocol-level and transport-agnostic, but this proves the NEW
//! `transport_kind = Unix` routing (added for this transport) doesn't
//! break it.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use tokio::io::split;

use shamir_connect::client::handshake::{HandshakeBuilder, ServerAuthOk, ServerChallenge};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::common::username::NormalizedUsername;

use shamir_transport_tcp::framing::{read_frame, write_frame, MAX_FRAME_SIZE_DEFAULT};

mod common;

#[derive(Serialize, Deserialize)]
struct WireAuthInit {
    user: String,
    #[serde(with = "serde_bytes")]
    client_nonce: Vec<u8>,
    binding_mode: u8,
    version: u8,
}

#[derive(Serialize, Deserialize)]
struct WireChallenge {
    #[serde(with = "serde_bytes")]
    salt: Vec<u8>,
    memory_kb: u32,
    time: u32,
    parallelism: u32,
    argon2_version: u8,
    #[serde(with = "serde_bytes")]
    server_nonce: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct WireClientProof {
    #[serde(with = "serde_bytes")]
    client_proof: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct WireAuthOk {
    #[serde(with = "serde_bytes")]
    server_signature: Vec<u8>,
    #[serde(with = "serde_bytes")]
    server_pub_key: Vec<u8>,
    #[serde(with = "serde_bytes")]
    identity_sig: Vec<u8>,
    #[serde(with = "serde_bytes")]
    session_id: Vec<u8>,
    expires_at_ns: u64,
    #[serde(default, with = "serde_bytes")]
    resumption_ticket: Vec<u8>,
    #[serde(default)]
    resumption_expires_at_ns: u64,
    #[serde(default)]
    server_query_version: u8,
}

#[derive(Serialize, Deserialize)]
struct WireResumeInit {
    #[serde(with = "serde_bytes")]
    ticket: Vec<u8>,
    #[serde(with = "serde_bytes")]
    client_nonce: Vec<u8>,
    binding_mode: u8,
}

#[derive(Serialize, Deserialize)]
struct WireResumeOk {
    #[serde(with = "serde_bytes")]
    session_id: Vec<u8>,
    expires_at_ns: u64,
    #[serde(default, with = "serde_bytes")]
    resumption_ticket: Vec<u8>,
    #[serde(default)]
    resumption_expires_at_ns: u64,
    #[serde(default)]
    server_query_version: u8,
}

#[cfg(unix)]
async fn connect_ipc(addr: &str) -> impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin {
    tokio::net::UnixStream::connect(addr)
        .await
        .expect("connect unix socket")
}

#[cfg(windows)]
async fn connect_ipc(addr: &str) -> impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin {
    tokio::net::windows::named_pipe::ClientOptions::new()
        .open(addr)
        .expect("open named pipe")
}

/// Perform a full SCRAM handshake over the unix transport and return
/// (session_id, resumption_ticket).
async fn do_full_auth<R, W>(
    r: &mut R,
    w: &mut W,
    username: &NormalizedUsername,
    password: &[u8],
) -> ([u8; 32], Vec<u8>)
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let hs = HandshakeBuilder::new(username.clone(), TransportKind::Unix, BindingMode::None)
        .accept_new_host(true)
        .build()
        .expect("handshake builder");

    let init = hs.auth_init();
    write_frame(
        w,
        &rmp_serde::to_vec(&WireAuthInit {
            user: init.user,
            client_nonce: init.client_nonce.to_vec(),
            binding_mode: init.binding_mode,
            version: init.version,
        })
        .unwrap(),
    )
    .await
    .expect("send auth_init");

    let ch_bytes = tokio::time::timeout(
        Duration::from_secs(30),
        read_frame(r, MAX_FRAME_SIZE_DEFAULT),
    )
    .await
    .expect("challenge within 30s")
    .expect("read challenge");
    let ch_wire: WireChallenge = rmp_serde::from_slice(&ch_bytes).unwrap();
    let mut salt = [0u8; 16];
    salt.copy_from_slice(&ch_wire.salt);
    let mut server_nonce = [0u8; 32];
    server_nonce.copy_from_slice(&ch_wire.server_nonce);
    let challenge = ServerChallenge {
        salt,
        kdf_params: KdfParams {
            memory_kb: ch_wire.memory_kb,
            time: ch_wire.time,
            parallelism: ch_wire.parallelism,
            argon2_version: ch_wire.argon2_version,
        },
        server_nonce,
    };

    let mut password_buf = password.to_vec();
    let (proof, derived, am) = hs
        .process_challenge(&challenge, &mut password_buf)
        .expect("process challenge");
    write_frame(
        w,
        &rmp_serde::to_vec(&WireClientProof {
            client_proof: proof.to_vec(),
        })
        .unwrap(),
    )
    .await
    .expect("send proof");

    let ok_bytes = tokio::time::timeout(
        Duration::from_secs(30),
        read_frame(r, MAX_FRAME_SIZE_DEFAULT),
    )
    .await
    .expect("auth_ok within 30s")
    .expect("read auth_ok");
    let ok_wire: WireAuthOk = rmp_serde::from_slice(&ok_bytes).unwrap();

    assert!(
        !ok_wire.resumption_ticket.is_empty(),
        "server must issue a resumption ticket over the unix transport"
    );

    let mut sig32 = [0u8; 32];
    sig32.copy_from_slice(&ok_wire.server_signature);
    let mut pub32 = [0u8; 32];
    pub32.copy_from_slice(&ok_wire.server_pub_key);
    let mut id_sig = [0u8; 64];
    id_sig.copy_from_slice(&ok_wire.identity_sig);
    let mut session_id = [0u8; 32];
    session_id.copy_from_slice(&ok_wire.session_id);

    let auth_ok = ServerAuthOk {
        server_signature: sig32,
        server_pub_key: pub32,
        identity_sig: id_sig,
        session_id,
        expires_at_ns: ok_wire.expires_at_ns,
        resumption_ticket: Some(ok_wire.resumption_ticket.clone()),
        resumption_expires_at_ns: Some(ok_wire.resumption_expires_at_ns),
        rotation_in_progress: None,
        kdf_upgrade_required: None,
    };
    hs.process_auth_ok(&auth_ok, &derived, &am, |_| {})
        .expect("process auth_ok");

    (session_id, ok_wire.resumption_ticket)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_after_full_auth_succeeds_over_unix_transport() {
    let temp = TempDir::new().expect("tempdir");
    let addr = common::ipc_test_addr(&temp);
    let password = b"correct horse battery staple".to_vec();
    let handle = common::spawn_ipc_with_password(&temp, &password, &addr).await;
    let server_addr = handle
        .first_ipc_path()
        .expect("the single unix listener must be bound")
        .to_string();
    let username = NormalizedUsername::from_raw("admin").expect("username");

    // ---- First connection: full SCRAM, capture ticket ----
    let stream1 = connect_ipc(&server_addr).await;
    let (mut r1, mut w1) = split(stream1);
    let (_session_id_1, ticket) = do_full_auth(&mut r1, &mut w1, &username, &password).await;
    drop(r1);
    drop(w1);

    // ---- Second, independent connection: resume with the ticket ----
    let stream2 = connect_ipc(&server_addr).await;
    let (mut r2, mut w2) = split(stream2);

    let mut client_nonce = [0u8; 32];
    shamir_connect::common::crypto::random_bytes(&mut client_nonce);

    let resume_init = WireResumeInit {
        ticket: ticket.clone(),
        client_nonce: client_nonce.to_vec(),
        binding_mode: BindingMode::None.as_u8(),
    };
    write_frame(&mut w2, &rmp_serde::to_vec(&resume_init).unwrap())
        .await
        .expect("send resume_init");

    let ok_bytes = tokio::time::timeout(
        Duration::from_secs(10),
        read_frame(&mut r2, MAX_FRAME_SIZE_DEFAULT),
    )
    .await
    .expect("resume_ok within 10s")
    .expect("read resume_ok");

    let ok: WireResumeOk = rmp_serde::from_slice(&ok_bytes).expect("decode ResumeOkWire");

    assert_eq!(ok.session_id.len(), 32, "session_id must be 32 bytes");
    assert!(ok.expires_at_ns > 0, "expires_at_ns must be non-zero");
    if !ok.resumption_ticket.is_empty() {
        assert!(
            ok.resumption_expires_at_ns > 0,
            "refresh ticket must carry a non-zero expiry"
        );
    }

    handle.shutdown().await;
}
