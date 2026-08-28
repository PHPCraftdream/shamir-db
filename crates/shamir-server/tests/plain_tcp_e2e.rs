//! Plain (no-TLS) TCP transport end-to-end test.
//!
//! Regression coverage for the `AcceptPath::TcpPlain` accept loop
//! (`server_launcher.rs`) — before it existed, `profile: plain` listeners
//! were silently skipped at boot (fell into the catch-all "unsupported MVP
//! combination" branch), so `ServerHandle::bound_addrs` held `None` for
//! them and no client could ever connect. This test boots a server with a
//! SINGLE `profile: plain` listener, connects with a bare `TcpStream` (no
//! TLS handshake at all), runs the full SCRAM-Argon2id handshake with
//! `binding_mode = 0x00` / zeroed channel binding, and executes one batch
//! against the database — proving the whole plain-transport path works,
//! not just that the socket accepts.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use tokio::io::split;
use tokio::net::TcpStream;

use shamir_connect::client::handshake::{HandshakeBuilder, ServerAuthOk, ServerChallenge};
use shamir_connect::common::crypto::sha256;
use shamir_connect::common::envelope::{RequestEnvelope, ResponseEnvelope};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::common::username::NormalizedUsername;

use shamir_transport_tcp::framing::{read_frame, write_frame, MAX_FRAME_SIZE_DEFAULT};

use shamir_server::db_handler::{DbRequest, DbResponse};

mod common;

// Wire-frame mirrors of the auth_init / challenge / client_proof / auth_ok
// envelopes — same shapes as `mvp_e2e.rs` (kept transport-binding-local).

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

async fn roundtrip<R, W>(
    req: &DbRequest,
    sid: [u8; 32],
    next_rid: &mut u32,
    w: &mut W,
    r: &mut R,
) -> DbResponse
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let bytes = rmp_serde::to_vec_named(req).expect("encode req");
    let rid = *next_rid;
    *next_rid += 1;
    let envelope = RequestEnvelope::new(sid, Some(rid), bytes);
    let envelope_bytes = envelope.to_msgpack().expect("envelope encode");
    write_frame(w, &envelope_bytes).await.expect("send request");
    let resp_bytes = tokio::time::timeout(
        Duration::from_secs(10),
        read_frame(r, MAX_FRAME_SIZE_DEFAULT),
    )
    .await
    .expect("response within 10s")
    .expect("read response");
    let resp_envelope = ResponseEnvelope::from_msgpack(&resp_bytes).expect("response envelope");
    assert_eq!(resp_envelope.request_id, Some(rid), "request_id echoed");
    rmp_serde::from_slice(&resp_envelope.res).expect("decode DbResponse")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plain_tcp_accept_loop_scram_batch() {
    let temp = TempDir::new().expect("tempdir");
    let password = b"correct horse battery staple".to_vec();
    let handle = common::spawn_plain_with_password(&temp, &password, "127.0.0.1:0").await;

    let server_addr = handle
        .first_tls_exporter_addr()
        .expect("the single plain listener must be bound (regression: was skipped pre-fix)");

    // -----------------------------------------------------------------
    // Client side: bare TCP, no TLS handshake at all.
    // -----------------------------------------------------------------
    let username = NormalizedUsername::from_raw("admin").expect("username");
    let tcp = TcpStream::connect(server_addr).await.expect("connect");
    let (mut r, mut w) = split(tcp);

    let pinned: Arc<std::sync::Mutex<Option<[u8; 32]>>> = Arc::new(std::sync::Mutex::new(None));
    let pinned_for_capture = pinned.clone();

    // `BindingMode::None` — no `.tls_exporter(...)` call, so the builder
    // keeps its zeroed default (matches server-side `[0u8; 32]` for plain).
    let hs = HandshakeBuilder::new(username.clone(), TransportKind::Tcp, BindingMode::None)
        .accept_new_host(true)
        .build()
        .expect("handshake builder");

    let init = hs.auth_init();
    let init_wire = WireAuthInit {
        user: init.user,
        client_nonce: init.client_nonce.to_vec(),
        binding_mode: init.binding_mode,
        version: init.version,
    };
    write_frame(&mut w, &rmp_serde::to_vec(&init_wire).unwrap())
        .await
        .expect("send auth_init");

    let ch_bytes = read_frame(&mut r, MAX_FRAME_SIZE_DEFAULT)
        .await
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

    let mut password_buf = password.clone();
    let (proof, derived, am) = hs
        .process_challenge(&challenge, &mut password_buf)
        .expect("process challenge");

    let proof_wire = WireClientProof {
        client_proof: proof.to_vec(),
    };
    write_frame(&mut w, &rmp_serde::to_vec(&proof_wire).unwrap())
        .await
        .expect("send proof");

    let ok_bytes = read_frame(&mut r, MAX_FRAME_SIZE_DEFAULT)
        .await
        .expect("read auth_ok");
    let ok_wire: WireAuthOk = rmp_serde::from_slice(&ok_bytes).unwrap();

    let mut sig32 = [0u8; 32];
    sig32.copy_from_slice(&ok_wire.server_signature);
    let mut pub32 = [0u8; 32];
    pub32.copy_from_slice(&ok_wire.server_pub_key);
    let mut id_sig = [0u8; 64];
    id_sig.copy_from_slice(&ok_wire.identity_sig);
    let mut session_id = [0u8; 32];
    session_id.copy_from_slice(&ok_wire.session_id);

    assert!(
        !ok_wire.resumption_ticket.is_empty(),
        "server must issue a resumption ticket in auth_ok even on the plain transport",
    );

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

    let success = hs
        .process_auth_ok(&auth_ok, &derived, &am, |pin| {
            *pinned_for_capture.lock().unwrap() = Some(*pin);
        })
        .expect("process auth_ok");
    assert_eq!(success.session_id, session_id);
    let captured = pinned.lock().unwrap().expect("TOFU callback fired");
    assert_eq!(captured, sha256(&pub32));

    // -----------------------------------------------------------------
    // Post-handshake: one real batch round-trip over the plain socket.
    // -----------------------------------------------------------------
    let mut next_rid: u32 = 1;
    let mut mk_batch = shamir_query_builder::batch::Batch::new();
    mk_batch.id("mk-db");
    mk_batch.create_db("mk", shamir_query_builder::ddl::create_db("prod"));
    let req = DbRequest::Execute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: mk_batch.build(),
    };
    let resp = roundtrip(&req, session_id, &mut next_rid, &mut w, &mut r).await;
    match resp {
        DbResponse::Batch { response } => {
            assert!(
                response.results.contains_key("mk"),
                "create_db ok over plain TCP"
            );
        }
        other => panic!("expected Batch, got {:?}", other),
    }

    handle.shutdown().await;
}
