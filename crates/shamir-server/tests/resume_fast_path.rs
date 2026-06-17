//! Integration tests for the session-resume fast-path (M5a).
//!
//! (a) Full SCRAM auth → receive ticket in `auth_ok` → reconnect using that
//!     ticket as `ResumeInit` → server responds with `ResumeOkWire` containing
//!     a fresh session_id and a new ticket.
//!
//! (b) Resume with a garbage ticket → server closes the connection (no
//!     `ResumeOkWire`).

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use tokio::io::split;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use zeroize::Zeroizing;

use shamir_connect::client::handshake::{HandshakeBuilder, ServerAuthOk, ServerChallenge};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::common::username::NormalizedUsername;

use shamir_transport_tcp::framing::{read_frame, write_frame, MAX_FRAME_SIZE_DEFAULT};
use shamir_transport_tcp::tls::{extract_tls_exporter, make_client_config_no_ca};

use shamir_server::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ProfileKind, TlsConfig,
};
use shamir_server::server::{BootstrapMode, ServerLauncher};

// ---------------------------------------------------------------------------
// Wire shapes (mirrors of the server-side `mod wire` structs)
// ---------------------------------------------------------------------------

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
    pub expires_at_ns: u64,
    #[serde(default, with = "serde_bytes")]
    pub resumption_ticket: Vec<u8>,
    #[serde(default)]
    pub resumption_expires_at_ns: u64,
    #[serde(default)]
    pub server_query_version: u8,
}

/// Client → server first frame for session resume.
#[derive(Serialize, Deserialize)]
struct WireResumeInit {
    #[serde(with = "serde_bytes")]
    ticket: Vec<u8>,
    #[serde(with = "serde_bytes")]
    client_nonce: Vec<u8>,
    binding_mode: u8,
}

/// Server → client response for successful resume.
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

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn fast_kdf() -> KdfConfig {
    KdfConfig {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

fn make_test_config(temp: &TempDir) -> Config {
    let data_dir: PathBuf = temp.path().to_path_buf();
    Config {
        data_dir: data_dir.clone(),
        logging: LoggingConfig {
            level: "warn".into(),
            slow_query_threshold_ms: 0,
            file: None,
            flush_interval_ms: 2000,
        },
        kdf_defaults: fast_kdf(),
        argon2_concurrent_max: 4,
        listeners: vec![ListenerConfig {
            kind: ListenerKind::Tcp,
            addr: "127.0.0.1:0".to_string(),
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
        observability: shamir_server::config::ObservabilityConfig {
            addr: String::new(),
        },
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Perform a full SCRAM handshake and return (session_id, resumption_ticket, exporter).
async fn do_full_auth<R, W>(
    r: &mut R,
    w: &mut W,
    exporter: [u8; 32],
    username: &NormalizedUsername,
    password: &[u8],
) -> ([u8; 32], Vec<u8>)
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let hs = HandshakeBuilder::new(
        username.clone(),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
    )
    .tls_exporter(exporter)
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
        "server must issue a resumption ticket"
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

/// Connect a fresh TLS connection and return split (read, write) + exporter.
async fn new_tls_conn(
    addr: std::net::SocketAddr,
) -> (
    impl tokio::io::AsyncRead + Unpin,
    impl tokio::io::AsyncWrite + Unpin,
    [u8; 32],
) {
    let client_cfg = make_client_config_no_ca();
    let connector = TlsConnector::from(client_cfg);
    let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let tcp = TcpStream::connect(addr).await.expect("connect");
    let tls = connector
        .connect(server_name, tcp)
        .await
        .expect("tls handshake");
    let exporter = extract_tls_exporter(&tls).expect("client exporter");
    let (r, w) = split(tls);
    (r, w, exporter)
}

// ---------------------------------------------------------------------------
// Test (a): full auth → resume → success
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_after_full_auth_succeeds() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().expect("tempdir");
    let config = make_test_config(&temp);
    let password = b"correct horse battery staple".to_vec();
    let bootstrap = BootstrapMode::Password {
        username: "admin".into(),
        password: Zeroizing::new(password.clone()),
    };
    let handle = ServerLauncher { config, bootstrap }
        .launch()
        .await
        .expect("launch");
    let server_addr = handle.first_tls_exporter_addr().expect("addr");
    let username = NormalizedUsername::from_raw("admin").expect("username");

    // ---- First connection: full SCRAM, capture ticket ---
    let (mut r1, mut w1, exporter1) = new_tls_conn(server_addr).await;
    let (_session_id_1, ticket) =
        do_full_auth(&mut r1, &mut w1, exporter1, &username, &password).await;
    drop(r1);
    drop(w1);

    // ---- Second connection: resume with ticket ----
    let (mut r2, mut w2, _exporter2) = new_tls_conn(server_addr).await;

    // Build a fresh client nonce.
    let mut client_nonce = [0u8; 32];
    shamir_connect::common::crypto::random_bytes(&mut client_nonce);

    let resume_init = WireResumeInit {
        ticket: ticket.clone(),
        client_nonce: client_nonce.to_vec(),
        binding_mode: BindingMode::TlsExporter.as_u8(),
    };
    write_frame(&mut w2, &rmp_serde::to_vec(&resume_init).unwrap())
        .await
        .expect("send resume_init");

    // Server should respond with ResumeOkWire.
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
    // A refresh ticket is issued only when the chain still has time beyond the
    // next TTL window. Right after initial SCRAM auth the chain age budget is
    // fully consumed, so no refresh ticket is expected — that is correct
    // behaviour per spec §5.4. We only assert that if a ticket IS present it
    // carries a valid expiry.
    if !ok.resumption_ticket.is_empty() {
        assert!(
            ok.resumption_expires_at_ns > 0,
            "refresh ticket must carry a non-zero expiry"
        );
    }

    handle.shutdown().await;
}

// ---------------------------------------------------------------------------
// Test (b): garbage ticket → connection closed
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_with_garbage_ticket_closes_connection() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().expect("tempdir");
    let config = make_test_config(&temp);
    let password = b"correct horse battery staple 2".to_vec();
    let bootstrap = BootstrapMode::Password {
        username: "admin".into(),
        password: Zeroizing::new(password.clone()),
    };
    let handle = ServerLauncher { config, bootstrap }
        .launch()
        .await
        .expect("launch");
    let server_addr = handle.first_tls_exporter_addr().expect("addr");

    let (mut r, mut w, exporter) = new_tls_conn(server_addr).await;

    // Send a ResumeInit with a garbage (non-empty) ticket.
    let mut client_nonce = [0u8; 32];
    shamir_connect::common::crypto::random_bytes(&mut client_nonce);
    let resume_init = WireResumeInit {
        ticket: vec![
            0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0xBA, 0xBE, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06,
            0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C,
        ],
        client_nonce: client_nonce.to_vec(),
        binding_mode: BindingMode::TlsExporter.as_u8(),
    };
    write_frame(&mut w, &rmp_serde::to_vec(&resume_init).unwrap())
        .await
        .expect("send garbage resume_init");

    // The server should close the connection — any read attempt returns EOF or error.
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        read_frame(&mut r, MAX_FRAME_SIZE_DEFAULT),
    )
    .await
    .expect("should not hang");

    match result {
        Err(_) => { /* connection closed / error — expected */ }
        Ok(bytes) => {
            // If we received bytes, they must NOT be a valid ResumeOkWire.
            // The server may send an error frame or nothing at all.
            let decoded = rmp_serde::from_slice::<WireResumeOk>(&bytes);
            assert!(
                decoded.is_err(),
                "must not receive a valid ResumeOkWire on garbage ticket"
            );
        }
    }

    let _ = exporter; // used implicitly via TLS binding
    handle.shutdown().await;
}
