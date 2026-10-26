//! E2E regression test for RV-CONNECT-0A42F42C (spec §7.4 overflow):
//! with 16 active sessions for a user, the 17th full login must be
//! REJECTED — the connection is closed with NO `auth_ok`, externally
//! indistinguishable from any other authentication failure (the old code
//! unconditionally evicted the LRU session and granted the login).

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use tokio::io::split;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use zeroize::Zeroizing;

use shamir_connect::client::handshake::{HandshakeBuilder, ServerChallenge};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::common::username::NormalizedUsername;

use shamir_transport_tcp::framing::{read_frame, write_frame, MAX_FRAME_SIZE_DEFAULT};
use shamir_transport_tcp::tls::{
    extract_tls_exporter, make_client_config_app_authenticated, AppLayerServerAuth,
};

use shamir_server::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ProfileKind, SecurityConfig,
    TlsConfig,
};
use shamir_server::server::{BootstrapMode, ServerLauncher};

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
    #[allow(dead_code)]
    server_signature: Vec<u8>,
    #[serde(with = "serde_bytes")]
    #[allow(dead_code)]
    server_pub_key: Vec<u8>,
    #[serde(with = "serde_bytes")]
    #[allow(dead_code)]
    identity_sig: Vec<u8>,
    #[serde(with = "serde_bytes")]
    #[allow(dead_code)]
    session_id: Vec<u8>,
    #[allow(dead_code)]
    pub expires_at_ns: u64,
    #[serde(default, with = "serde_bytes")]
    #[allow(dead_code)]
    pub resumption_ticket: Vec<u8>,
    #[serde(default)]
    #[allow(dead_code)]
    pub resumption_expires_at_ns: u64,
    #[serde(default)]
    #[allow(dead_code)]
    pub server_query_version: u8,
}

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
    let data_dir_clone = data_dir.clone();
    Config {
        durability: Default::default(),
        transactions: Default::default(),
        data_dir: data_dir_clone,
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
        // 17 logins arrive from one subnet within a second; the default
        // 10/s auth_init bucket would close some before the challenge.
        security: SecurityConfig {
            auth_init_rate_per_second: 1000,
            ..Default::default()
        },
        audit: Default::default(),
        observability: shamir_server::config::ObservabilityConfig {
            addr: String::new(),
            allow_public_metrics: false,
        },
        replication: None,
    }
}

/// Fresh TLS connection + exporter.
async fn new_tls_conn(
    addr: std::net::SocketAddr,
) -> (
    impl tokio::io::AsyncRead + Unpin,
    impl tokio::io::AsyncWrite + Unpin,
    [u8; 32],
) {
    let client_cfg =
        make_client_config_app_authenticated(AppLayerServerAuth::exporter_bound()).unwrap();
    let connector = TlsConnector::from(client_cfg);
    let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let tcp = TcpStream::connect(addr).await.expect("connect");
    let tls = connector.connect(server_name, tcp).await.expect("tls");
    let exporter = extract_tls_exporter(&tls).expect("client exporter");
    let (r, w) = split(tls);
    (r, w, exporter)
}

/// Run the client side of a full SCRAM login. `expect_ok == true` asserts
/// the server answers with a decodable `auth_ok`; `expect_ok == false`
/// asserts the server NEVER sends `auth_ok` (closure / garbage instead).
async fn full_login(
    addr: std::net::SocketAddr,
    username: &NormalizedUsername,
    password: &[u8],
    expect_ok: bool,
) {
    let (mut r, mut w, exporter) = new_tls_conn(addr).await;

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
        &mut w,
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
        read_frame(&mut r, MAX_FRAME_SIZE_DEFAULT),
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
    let (proof, _derived, _am) = hs
        .process_challenge(&challenge, &mut password_buf)
        .expect("process challenge");
    write_frame(
        &mut w,
        &rmp_serde::to_vec(&WireClientProof {
            client_proof: proof.to_vec(),
        })
        .unwrap(),
    )
    .await
    .expect("send proof");

    let resp = tokio::time::timeout(
        Duration::from_secs(30),
        read_frame(&mut r, MAX_FRAME_SIZE_DEFAULT),
    )
    .await;
    if expect_ok {
        let ok_bytes = resp
            .expect("auth_ok within 30s")
            .expect("server must not close a valid login");
        let ok: WireAuthOk = rmp_serde::from_slice(&ok_bytes).expect("decode auth_ok");
        assert_eq!(ok.session_id.len(), 32, "auth_ok must carry a session id");
    } else {
        match resp {
            // Connection closed (EOF) with NO auth_ok frame: the required
            // generic-failure behaviour.
            Err(_) => panic!("17th login timed out instead of closing"),
            Ok(Err(_)) => {}
            Ok(Ok(bytes)) => panic!(
                "17th login must be closed without any frame, got {} bytes",
                bytes.len()
            ),
        }
    }
    drop(r);
    drop(w);
}

/// RV-CONNECT-0A42F42C — 16 ACTIVE sessions exist for the user (each of
/// the 16 successful logins created one moments ago, well inside the 60 s
/// activity window); the 17th login must be rejected: connection closed,
/// NO `auth_ok` (spec §7.4 all-active rejection; internal audit reason
/// `max_sessions`, wire-generic failure).
///
/// AUDIT CAPTURE LIMITATION (explicit, not faked): the internal audit chain
/// is written by `FjallAuditAppender` into the server's Fjall keyspace
/// inside `data_dir` (batched writes) — `AuditConfig` exposes only rotation
/// size/retention, NOT a file path, so this black-box harness cannot read
/// the `auth_failed/max_sessions` entry or count `auth_success` events.
/// The internal-reason assertion therefore lives in
/// `shamir-server/src/sync/connection/handshake.rs` (code path) +
/// `admission_cap_tests.rs`/`resume_cap_tests.rs` (unit level); this test
/// asserts only the externally observable closure.
///
/// Fails if ONLY the all-active rejection fix is reverted (old
/// `insert_with_per_user_cap` evicts the LRU session without any activity
/// threshold): the 17th login receives `auth_ok` and succeeds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rv_connect_0a42f42c_capped_login_rejected_when_all_active() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().expect("tempdir");
    let config = make_test_config(&temp);
    let password = b"correct horse battery staple 3".to_vec();
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

    // 16 successful full logins → 16 ACTIVE sessions for "admin" (created
    // just now, so all are inside the 60 s activity window).
    for _ in 0..16 {
        full_login(server_addr, &username, &password, true).await;
    }

    // 17th login while ALL 16 sessions are active → rejected, closed, no
    // auth_ok.
    full_login(server_addr, &username, &password, false).await;

    handle.shutdown().await;
}
