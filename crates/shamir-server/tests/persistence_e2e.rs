//! Persistence E2E test — proves data written through the wire survives
//! a full server restart.
//!
//! Strategy:
//!   1. Boot ServerLauncher#1 with a fresh TempDir.
//!   2. Real client connects, authenticates, writes a record into the
//!      pre-configured `default.main.items` table via Batch API.
//!   3. Cleanly shutdown ServerLauncher#1.
//!   4. Boot ServerLauncher#2 with the SAME data_dir (which already has
//!      durable redb files for: server_meta, users, counters, audit_log,
//!      shamir_db_meta, shamir_db_default_main).
//!   5. Same client (same admin password) connects again, reads the row
//!      back, asserts the value matches what was written.
//!
//! The bootstrap helper is idempotent — second boot finds the admin user
//! already in the directory and is a no-op. The TLS cert/key are loaded
//! from disk on the second boot (no re-generation), so the Ed25519 server
//! identity is stable across restarts (TOFU pin from boot #1 is still
//! valid on boot #2).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tempfile::TempDir;
use tokio::io::{split, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use zeroize::Zeroizing;

use shamir_connect::client::handshake::{HandshakeBuilder, ServerAuthOk, ServerChallenge};
use shamir_connect::common::envelope::{RequestEnvelope, ResponseEnvelope};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::common::username::NormalizedUsername;

use shamir_transport_tcp::framing::{read_frame, write_frame, MAX_FRAME_SIZE_DEFAULT};
use shamir_transport_tcp::tls::{extract_tls_exporter, make_client_config_no_ca};

use shamir_server::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ProfileKind, TlsConfig,
};
use shamir_server::db_handler::{DbRequest, DbResponse};
use shamir_server::server::{BootstrapMode, ServerHandle, ServerLauncher};

// --------------------------------------------------------------------------
// Shared wire-frame mirrors (same as mvp_e2e.rs)
// --------------------------------------------------------------------------

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
}

// --------------------------------------------------------------------------
// Fixtures
// --------------------------------------------------------------------------

fn fast_kdf() -> KdfConfig {
    KdfConfig {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

/// Build a config that targets a specific port. Used for the second boot
/// so we can reuse the same address even though the first boot used 0
/// (OS-picked).
fn make_config(data_dir: PathBuf, port: u16) -> Config {
    Config {
        data_dir: data_dir.clone(),
        logging: LoggingConfig { level: "warn".into(), slow_query_threshold_ms: 0 },
        kdf_defaults: fast_kdf(),
        argon2_concurrent_max: 4,
        listeners: vec![ListenerConfig {
            kind: ListenerKind::Tcp,
            addr: format!("127.0.0.1:{port}"),
            profile: ProfileKind::TlsExporter,
            path: None,
            kdf_override: None,
            browser_origin_allowlist: vec![],
        }],
        tls: TlsConfig {
            cert_path: data_dir.join("cert.pem"),
            key_path: data_dir.join("key.pem"),
        },
        security: Default::default(),
        audit: Default::default(),
    }
}

// --------------------------------------------------------------------------
// Helpers
// --------------------------------------------------------------------------

async fn launch(data_dir: PathBuf, port: u16, password: &[u8]) -> ServerHandle {
    let bootstrap = BootstrapMode::Password {
        username: "admin".into(),
        password: Zeroizing::new(password.to_vec()),
    };
    ServerLauncher {
        config: make_config(data_dir, port),
        bootstrap,
    }
    .launch()
    .await
    .expect("launcher boot")
}

/// Drive a SCRAM client through to authentication, returning the open
/// connection halves and the session_id ready for post-auth use.
async fn login(
    addr: std::net::SocketAddr,
    password: &[u8],
) -> (
    tokio::io::ReadHalf<tokio_rustls::client::TlsStream<TcpStream>>,
    tokio::io::WriteHalf<tokio_rustls::client::TlsStream<TcpStream>>,
    [u8; 32],
) {
    let username = NormalizedUsername::from_raw("admin").expect("username");
    let client_cfg = make_client_config_no_ca();
    let connector = TlsConnector::from(client_cfg);
    let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let tcp = TcpStream::connect(addr).await.expect("tcp connect");
    let tls = connector
        .connect(server_name, tcp)
        .await
        .expect("tls handshake");
    let exporter = extract_tls_exporter(&tls).expect("exporter");

    let (mut r, mut w) = split(tls);

    let hs = HandshakeBuilder::new(
        username.clone(),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
    )
    .tls_exporter(exporter)
    .accept_new_host(true)
    .build()
    .expect("handshake builder");

    // auth_init
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

    // challenge
    let ch_bytes = read_frame(&mut r, MAX_FRAME_SIZE_DEFAULT).await.expect("read challenge");
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

    // proof
    let mut password_buf = password.to_vec();
    let (proof, derived, am) = hs
        .process_challenge(&challenge, &mut password_buf)
        .expect("process challenge");
    let proof_wire = WireClientProof { client_proof: proof.to_vec() };
    write_frame(&mut w, &rmp_serde::to_vec(&proof_wire).unwrap())
        .await
        .expect("send proof");

    // auth_ok
    let ok_bytes = read_frame(&mut r, MAX_FRAME_SIZE_DEFAULT).await.expect("read auth_ok");
    let ok_wire: WireAuthOk = rmp_serde::from_slice(&ok_bytes).unwrap();
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
    let _success = hs
        .process_auth_ok(&auth_ok, &derived, &am, |_pin| {})
        .expect("process auth_ok");

    (r, w, session_id)
}

async fn roundtrip<R, W>(
    req: &DbRequest,
    sid: [u8; 32],
    rid: u32,
    w: &mut W,
    r: &mut R,
) -> DbResponse
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let bytes = rmp_serde::to_vec_named(req).expect("encode req");
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
    let resp_envelope =
        ResponseEnvelope::from_msgpack(&resp_bytes).expect("response envelope");
    rmp_serde::from_slice(&resp_envelope.res).expect("decode DbResponse")
}

fn create_table_req(name: &str) -> DbRequest {
    let batch: shamir_db::db::query::batch::BatchRequest = serde_json::from_value(json!({
        "id": "tbl",
        "queries": { "tb": { "create_table": name, "repo": "main" } }
    }))
    .expect("parse batch");
    DbRequest::Execute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch,
    }
}

fn write_req(table: &str, sku: &str, qty: i64) -> DbRequest {
    let batch: shamir_db::db::query::batch::BatchRequest = serde_json::from_value(json!({
        "id": "wr",
        "queries": {
            "ins": { "set": table, "key": {"sku": sku}, "value": {"sku": sku, "qty": qty} }
        }
    }))
    .expect("parse batch");
    DbRequest::Execute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch,
    }
}

fn read_req(table: &str) -> DbRequest {
    let batch: shamir_db::db::query::batch::BatchRequest = serde_json::from_value(json!({
        "id": "rd",
        "queries": { "rd": { "from": table } }
    }))
    .expect("parse batch");
    DbRequest::Execute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch,
    }
}

// --------------------------------------------------------------------------
// The test
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn data_written_via_wire_survives_restart() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().expect("tempdir");
    let data_dir = temp.path().to_path_buf();
    let password: &[u8] = b"correct horse battery staple";

    // ----- BOOT #1: create table + write data -----
    let port: u16 = {
        let handle = launch(data_dir.clone(), 0, password).await;
        let addr = handle.first_tls_exporter_addr().expect("bound");

        let (mut r, mut w, sid) = login(addr, password).await;

        // create_table requires superuser — admin is bootstrapped as such.
        let create = roundtrip(&create_table_req("widgets"), sid, 1, &mut w, &mut r).await;
        match &create {
            DbResponse::Batch { .. } => {}
            other => panic!("create_table failed: {:?}", other),
        }

        let written =
            roundtrip(&write_req("widgets", "X1", 42), sid, 2, &mut w, &mut r).await;
        match &written {
            DbResponse::Batch { response } => {
                assert!(response.results.contains_key("ins"));
            }
            other => panic!("write failed: {:?}", other),
        }

        // Read back inside the same boot to confirm the path works.
        let read = roundtrip(&read_req("widgets"), sid, 3, &mut w, &mut r).await;
        match &read {
            DbResponse::Batch { response } => {
                let rd = response.results.get("rd").expect("rd alias");
                assert_eq!(rd.records.len(), 1);
                assert_eq!(rd.records[0].get("sku").and_then(|v| v.as_str()), Some("X1"));
                assert_eq!(rd.records[0].get("qty").and_then(|v| v.as_i64()), Some(42));
            }
            other => panic!("read failed inside boot #1: {:?}", other),
        }

        let _ = w.shutdown().await;
        let mut tmp = [0u8; 1];
        let _ = r.read(&mut tmp).await;
        let port = addr.port();
        handle.shutdown().await;
        port
    };

    // ----- BOOT #2: same data_dir, expect data + table to still be there -----
    let handle = launch(data_dir.clone(), port, password).await;
    let addr = handle.first_tls_exporter_addr().expect("bound after restart");
    assert_eq!(addr.port(), port, "second boot must reuse the same port");

    let (mut r, mut w, sid) = login(addr, password).await;
    let read = roundtrip(&read_req("widgets"), sid, 1, &mut w, &mut r).await;
    match &read {
        DbResponse::Batch { response } => {
            let rd = response
                .results
                .get("rd")
                .expect("rd alias must exist after restart");
            assert_eq!(
                rd.records.len(),
                1,
                "exactly one record must survive restart; got {}",
                rd.records.len()
            );
            assert_eq!(
                rd.records[0].get("sku").and_then(|v| v.as_str()),
                Some("X1"),
                "sku must match what was written in boot #1"
            );
            assert_eq!(
                rd.records[0].get("qty").and_then(|v| v.as_i64()),
                Some(42),
                "qty must match what was written in boot #1"
            );
        }
        DbResponse::Error { code, message } => {
            panic!("expected persisted data, got error {{ code: {code:?}, message: {message:?} }}");
        }
        other => panic!("expected Batch on second boot read, got {:?}", other),
    }

    let _ = w.shutdown().await;
    let mut tmp = [0u8; 1];
    let _ = r.read(&mut tmp).await;
    handle.shutdown().await;
}
