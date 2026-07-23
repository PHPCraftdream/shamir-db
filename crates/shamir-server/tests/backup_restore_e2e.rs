//! End-to-end backup → restore round-trip.
//!
//! Strategy:
//!   1. Boot ServerLauncher#1 against a fresh data_dir.
//!   2. Real client logs in, writes a row.
//!   3. Cleanly shutdown.
//!   4. `backup::backup(data_dir, snapshot_dir)` — copies every file.
//!   5. **Wipe the original data_dir** (simulating disk loss).
//!   6. **Restore**: copy the backup back into a fresh data_dir.
//!   7. Boot ServerLauncher#2 with the restored data_dir.
//!   8. Same client (same admin password) logs in, reads the row, asserts
//!      it matches what was written in boot #1.
//!
//! This proves the WHOLE backup pipeline is sound — not just the file-copy
//! mechanics (which `backup::tests` already cover) but every durable
//! piece the server depends on:
//!   * server_meta.redb     (Ed25519 identity, audit chain key, ticket key)
//!   * users.redb           (admin SCRAM credentials)
//!   * counters.redb        (replay counters)
//!   * shamir_db_meta.redb  (database / repo metadata)
//!   * shamir_db_default_main.redb (the data we wrote)
//!   * wire_tables.mpack    (registry of tables created over the wire)
//!   * cert.pem / key.pem   (TLS material — without it the second boot
//!   * cert.pem / key.pem   (TLS material — without it the second boot
//!     would generate a NEW Ed25519 + new server pub-key,
//!     breaking client TOFU pin sanity)

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;
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

use shamir_server::backup;
use shamir_server::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ObservabilityConfig,
    ProfileKind, TlsConfig,
};
use shamir_server::db_handler::{DbRequest, DbResponse};
use shamir_server::restore;
use shamir_server::server::{BootstrapMode, ServerHandle, ServerLauncher};

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

fn fast_kdf() -> KdfConfig {
    KdfConfig {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

fn make_config(data_dir: PathBuf, port: u16) -> Config {
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
        observability: ObservabilityConfig {
            addr: String::new(),
            allow_public_metrics: false,
        },
        replication: None,
    }
}

async fn launch(data_dir: PathBuf, port: u16, password: &[u8]) -> ServerHandle {
    ServerLauncher {
        config: make_config(data_dir, port),
        bootstrap: BootstrapMode::Password {
            username: "admin".into(),
            password: Zeroizing::new(password.to_vec()),
        },
    }
    .launch()
    .await
    .expect("launcher boot")
}

async fn login(
    addr: std::net::SocketAddr,
    password: &[u8],
) -> (
    tokio::io::ReadHalf<tokio_rustls::client::TlsStream<TcpStream>>,
    tokio::io::WriteHalf<tokio_rustls::client::TlsStream<TcpStream>>,
    [u8; 32],
) {
    let username = NormalizedUsername::from_raw("admin").unwrap();
    let connector = TlsConnector::from(make_client_config_no_ca());
    let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let tcp = TcpStream::connect(addr).await.unwrap();
    let tls = connector.connect(server_name, tcp).await.unwrap();
    let exporter = extract_tls_exporter(&tls).unwrap();
    let (mut r, mut w) = split(tls);

    let hs = HandshakeBuilder::new(username, TransportKind::Tcp, BindingMode::TlsExporter)
        .tls_exporter(exporter)
        .accept_new_host(true)
        .build()
        .unwrap();
    let init = hs.auth_init();
    let init_wire = WireAuthInit {
        user: init.user,
        client_nonce: init.client_nonce.to_vec(),
        binding_mode: init.binding_mode,
        version: init.version,
    };
    write_frame(&mut w, &rmp_serde::to_vec(&init_wire).unwrap())
        .await
        .unwrap();

    let ch_bytes = read_frame(&mut r, MAX_FRAME_SIZE_DEFAULT).await.unwrap();
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
    let (proof, derived, am) = hs.process_challenge(&challenge, &mut password_buf).unwrap();
    write_frame(
        &mut w,
        &rmp_serde::to_vec(&WireClientProof {
            client_proof: proof.to_vec(),
        })
        .unwrap(),
    )
    .await
    .unwrap();

    let ok_bytes = read_frame(&mut r, MAX_FRAME_SIZE_DEFAULT).await.unwrap();
    let ok_wire: WireAuthOk = rmp_serde::from_slice(&ok_bytes).unwrap();
    let mut sig = [0u8; 32];
    sig.copy_from_slice(&ok_wire.server_signature);
    let mut pub32 = [0u8; 32];
    pub32.copy_from_slice(&ok_wire.server_pub_key);
    let mut id_sig = [0u8; 64];
    id_sig.copy_from_slice(&ok_wire.identity_sig);
    let mut session_id = [0u8; 32];
    session_id.copy_from_slice(&ok_wire.session_id);
    let auth_ok = ServerAuthOk {
        server_signature: sig,
        server_pub_key: pub32,
        identity_sig: id_sig,
        session_id,
        expires_at_ns: ok_wire.expires_at_ns,
        resumption_ticket: Some(ok_wire.resumption_ticket.clone()),
        resumption_expires_at_ns: Some(ok_wire.resumption_expires_at_ns),
        rotation_in_progress: None,
        kdf_upgrade_required: None,
    };
    let pinned: Arc<std::sync::Mutex<Option<[u8; 32]>>> = Arc::new(std::sync::Mutex::new(None));
    let pinned2 = pinned.clone();
    hs.process_auth_ok(&auth_ok, &derived, &am, |p| {
        *pinned2.lock().unwrap() = Some(*p);
    })
    .unwrap();
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
    let bytes = rmp_serde::to_vec_named(req).unwrap();
    let envelope = RequestEnvelope::new(sid, Some(rid), bytes);
    write_frame(w, &envelope.to_msgpack().unwrap())
        .await
        .unwrap();
    let resp_bytes = tokio::time::timeout(
        Duration::from_secs(10),
        read_frame(r, MAX_FRAME_SIZE_DEFAULT),
    )
    .await
    .unwrap()
    .unwrap();
    let resp = ResponseEnvelope::from_msgpack(&resp_bytes).unwrap();
    rmp_serde::from_slice(&resp.res).unwrap()
}

fn create_table_req(name: &str) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("tbl");
    b.create_table("tb", shamir_query_builder::ddl::create_table(name));
    DbRequest::Execute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: b.build(),
    }
}
fn write_req(table: &str, sku: &str, qty: i64) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("wr");
    b.upsert(
        "ins",
        shamir_query_builder::write::upsert(table)
            .key(mpack!({"sku": @(QueryValue::from(sku))}))
            .value(shamir_query_builder::doc! { "sku" => sku, "qty" => qty }),
    );
    DbRequest::Execute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: b.build(),
    }
}
fn read_req(table: &str) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("rd");
    b.query("rd", shamir_query_builder::Query::from(table));
    DbRequest::Execute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: b.build(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn backup_then_restore_recovers_data() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let password: &[u8] = b"correct horse battery staple";

    // ---- Boot #1: create + write ----
    let data_temp = TempDir::new().unwrap();
    let data_dir = data_temp.path().to_path_buf();

    let port: u16 = {
        let handle = launch(data_dir.clone(), 0, password).await;
        let addr = handle.first_tls_exporter_addr().unwrap();
        let port = addr.port();

        let (mut r, mut w, sid) = login(addr, password).await;

        let res = roundtrip(&create_table_req("backup_items"), sid, 1, &mut w, &mut r).await;
        assert!(matches!(res, DbResponse::Batch { .. }), "create_table");

        let res = roundtrip(&write_req("backup_items", "X1", 42), sid, 2, &mut w, &mut r).await;
        assert!(matches!(res, DbResponse::Batch { .. }), "write");

        let _ = w.shutdown().await;
        let mut tmp = [0u8; 1];
        let _ = r.read(&mut tmp).await;
        handle.shutdown().await;
        // Give the OS a moment to settle pending fsyncs / file-handle drops.
        // The Drop chain (Arc<ShamirDb> → DbInstance → RepoInstance →
        // redb Database) closes the redb files; copying them only after
        // every Drop has run is the only way to guarantee the snapshot
        // sees the post-write state on Windows where mmap regions linger.
        tokio::time::sleep(Duration::from_millis(200)).await;
        port
    };

    // Diagnostic: list what's in data_dir before backup so a regression
    // shows which file is missing.
    let pre_backup_files: Vec<String> = std::fs::read_dir(&data_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    eprintln!("data_dir before backup: {:?}", pre_backup_files);
    let wt = std::fs::read_to_string(data_dir.join("wire_tables.mpack")).unwrap_or_default();
    eprintln!("wire_tables.mpack: {}", wt);
    let main_size = std::fs::metadata(data_dir.join("shamir_db_default_main.redb"))
        .map(|m| m.len())
        .unwrap_or(0);
    eprintln!("default_main.redb size: {} bytes", main_size);

    // ---- Backup ----
    let backup_dest = TempDir::new().unwrap();
    let report = backup::backup(&data_dir, backup_dest.path()).expect("backup ok");
    assert!(
        report.files_copied > 0,
        "backup must have copied at least one file"
    );

    // ---- "Disaster" — wipe data_dir contents (but keep the directory
    //      itself, so we restore back to the SAME path). ----
    //
    // Restoring to the SAME absolute path is required because ShamirDb's
    // system_store records each repo's data file as an absolute path. A
    // future patch could rewrite paths on boot to make migration to a
    // different `data_dir` work; for now the disaster-recovery contract
    // is "stop server, restore files to the same place, restart".
    for entry in std::fs::read_dir(&data_dir).unwrap() {
        let entry = entry.unwrap();
        let p = entry.path();
        if p.is_dir() {
            std::fs::remove_dir_all(&p).unwrap();
        } else {
            std::fs::remove_file(&p).unwrap();
        }
    }
    // Sanity: data_dir is now empty.
    assert_eq!(std::fs::read_dir(&data_dir).unwrap().count(), 0);

    // ---- Restore: copy snapshot back into the same data_dir ----
    fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let path = entry.path();
            let target = dst.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                copy_dir_recursive(&path, &target)?;
            } else {
                std::fs::copy(&path, &target)?;
            }
        }
        Ok(())
    }
    copy_dir_recursive(&report.dest_dir, &data_dir).expect("restore copy");

    // ---- Boot #2 against the restored (same) data_dir ----
    let handle = launch(data_dir.clone(), port, password).await;
    let addr = handle.first_tls_exporter_addr().unwrap();
    assert_eq!(addr.port(), port, "second boot reuses same port");

    let (mut r, mut w, sid) = login(addr, password).await;
    let res = roundtrip(&read_req("backup_items"), sid, 1, &mut w, &mut r).await;
    match res {
        DbResponse::Batch { response } => {
            let rd = response
                .results
                .get("rd")
                .expect("rd alias must exist after restore");
            assert_eq!(
                rd.records.len(),
                1,
                "exactly one record must survive backup → wipe → restore"
            );
            assert_eq!(rd.records[0].get_value_str("sku"), Some("X1"));
            assert_eq!(rd.records[0].get_value_i64("qty"), Some(42));
        }
        DbResponse::Error { code, message } => {
            panic!("expected restored data, got error {{ code: {code:?}, message: {message:?} }}");
        }
        other => panic!("expected Batch, got {:?}", other),
    }

    let _ = w.shutdown().await;
    let mut tmp = [0u8; 1];
    let _ = r.read(&mut tmp).await;
    handle.shutdown().await;
}

// ----------------------------------------------------------------------------
// RI-11: full restore-CLI flow — manifest verify, atomic swap,
// `.pre_restore_backup_*` rollback path, ticket invalidation.
// ----------------------------------------------------------------------------

/// Client → server first frame for session resume (mirrors
/// `tests/resume_fast_path.rs`'s wire shape).
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

/// Same handshake as `login`, but also returns the raw resumption ticket
/// bytes from `AuthOk` so the caller can attempt to resume with it later.
async fn login_capture_ticket(
    addr: std::net::SocketAddr,
    password: &[u8],
) -> (
    tokio::io::ReadHalf<tokio_rustls::client::TlsStream<TcpStream>>,
    tokio::io::WriteHalf<tokio_rustls::client::TlsStream<TcpStream>>,
    [u8; 32],
    Vec<u8>,
) {
    let username = NormalizedUsername::from_raw("admin").unwrap();
    let connector = TlsConnector::from(make_client_config_no_ca());
    let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let tcp = TcpStream::connect(addr).await.unwrap();
    let tls = connector.connect(server_name, tcp).await.unwrap();
    let exporter = extract_tls_exporter(&tls).unwrap();
    let (mut r, mut w) = split(tls);

    let hs = HandshakeBuilder::new(username, TransportKind::Tcp, BindingMode::TlsExporter)
        .tls_exporter(exporter)
        .accept_new_host(true)
        .build()
        .unwrap();
    let init = hs.auth_init();
    let init_wire = WireAuthInit {
        user: init.user,
        client_nonce: init.client_nonce.to_vec(),
        binding_mode: init.binding_mode,
        version: init.version,
    };
    write_frame(&mut w, &rmp_serde::to_vec(&init_wire).unwrap())
        .await
        .unwrap();

    let ch_bytes = read_frame(&mut r, MAX_FRAME_SIZE_DEFAULT).await.unwrap();
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
    let (proof, derived, am) = hs.process_challenge(&challenge, &mut password_buf).unwrap();
    write_frame(
        &mut w,
        &rmp_serde::to_vec(&WireClientProof {
            client_proof: proof.to_vec(),
        })
        .unwrap(),
    )
    .await
    .unwrap();

    let ok_bytes = read_frame(&mut r, MAX_FRAME_SIZE_DEFAULT).await.unwrap();
    let ok_wire: WireAuthOk = rmp_serde::from_slice(&ok_bytes).unwrap();
    let mut sig = [0u8; 32];
    sig.copy_from_slice(&ok_wire.server_signature);
    let mut pub32 = [0u8; 32];
    pub32.copy_from_slice(&ok_wire.server_pub_key);
    let mut id_sig = [0u8; 64];
    id_sig.copy_from_slice(&ok_wire.identity_sig);
    let mut session_id = [0u8; 32];
    session_id.copy_from_slice(&ok_wire.session_id);
    let auth_ok = ServerAuthOk {
        server_signature: sig,
        server_pub_key: pub32,
        identity_sig: id_sig,
        session_id,
        expires_at_ns: ok_wire.expires_at_ns,
        resumption_ticket: Some(ok_wire.resumption_ticket.clone()),
        resumption_expires_at_ns: Some(ok_wire.resumption_expires_at_ns),
        rotation_in_progress: None,
        kdf_upgrade_required: None,
    };
    let pinned: Arc<std::sync::Mutex<Option<[u8; 32]>>> = Arc::new(std::sync::Mutex::new(None));
    let pinned2 = pinned.clone();
    hs.process_auth_ok(&auth_ok, &derived, &am, |p| {
        *pinned2.lock().unwrap() = Some(*p);
    })
    .unwrap();

    assert!(
        !ok_wire.resumption_ticket.is_empty(),
        "server must issue a resumption ticket on full auth"
    );
    (r, w, session_id, ok_wire.resumption_ticket)
}

/// Attempt to resume a session with `ticket` on a fresh connection. Returns
/// `true` iff the server responded with a well-formed `ResumeOkWire`;
/// `false` if the connection was closed or the response did not decode as
/// one (both are "resume rejected" from the caller's point of view).
async fn attempt_resume(addr: std::net::SocketAddr, ticket: Vec<u8>) -> bool {
    let (mut r, mut w, _exporter) = {
        let connector = TlsConnector::from(make_client_config_no_ca());
        let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
        let tcp = TcpStream::connect(addr).await.unwrap();
        let tls = connector.connect(server_name, tcp).await.unwrap();
        let exporter = extract_tls_exporter(&tls).unwrap();
        let (r, w) = split(tls);
        (r, w, exporter)
    };

    let mut client_nonce = [0u8; 32];
    shamir_connect::common::crypto::random_bytes(&mut client_nonce);
    let resume_init = WireResumeInit {
        ticket,
        client_nonce: client_nonce.to_vec(),
        binding_mode: BindingMode::TlsExporter.as_u8(),
    };
    write_frame(&mut w, &rmp_serde::to_vec(&resume_init).unwrap())
        .await
        .expect("send resume_init");

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        read_frame(&mut r, MAX_FRAME_SIZE_DEFAULT),
    )
    .await
    .expect("should not hang");

    match result {
        Err(_) => false, // connection closed / read error — rejected
        Ok(bytes) => rmp_serde::from_slice::<WireResumeOk>(&bytes).is_ok(),
    }
}

/// Full brief-described flow (RI-11 Part 4):
///   boot #1 → write data → obtain a resumption ticket → shutdown →
///   `backup::backup` → boot #2 → write MORE data → shutdown →
///   `restore::restore` (pointing at the boot #1 snapshot) → boot #3 →
///   assert the boot #2 writes are GONE (rolled back to the snapshot) →
///   assert the OLD ticket (from boot #1) is now REJECTED → assert a
///   `.pre_restore_backup_*` sibling directory exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restore_cli_rolls_back_data_and_invalidates_tickets() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let password: &[u8] = b"correct horse battery staple RI-11";

    let data_temp = TempDir::new().unwrap();
    let data_dir = data_temp.path().to_path_buf();

    // ---- Boot #1: create table, write ONE row, capture a resumption ticket ----
    let (port, old_ticket) = {
        let handle = launch(data_dir.clone(), 0, password).await;
        let addr = handle.first_tls_exporter_addr().unwrap();
        let port = addr.port();

        let (mut r, mut w, sid, ticket) = login_capture_ticket(addr, password).await;

        let res = roundtrip(&create_table_req("ri11_items"), sid, 1, &mut w, &mut r).await;
        assert!(matches!(res, DbResponse::Batch { .. }), "create_table");
        let res = roundtrip(&write_req("ri11_items", "PRE", 1), sid, 2, &mut w, &mut r).await;
        assert!(matches!(res, DbResponse::Batch { .. }), "pre-backup write");

        let _ = w.shutdown().await;
        let mut tmp = [0u8; 1];
        let _ = r.read(&mut tmp).await;
        handle.shutdown().await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        (port, ticket)
    };

    // ---- Backup: snapshot the state as of boot #1 ----
    let backup_dest = TempDir::new().unwrap();
    let report = backup::backup(&data_dir, backup_dest.path()).expect("backup ok");
    assert!(
        report.files_copied > 0,
        "backup must copy at least one file"
    );
    assert!(
        report.manifest_path.exists(),
        "backup must write manifest.json"
    );

    // ---- Boot #2: write MORE data so post-backup state differs from the snapshot ----
    {
        let handle = launch(data_dir.clone(), port, password).await;
        let addr = handle.first_tls_exporter_addr().unwrap();
        let (mut r, mut w, sid) = login(addr, password).await;

        let res = roundtrip(&write_req("ri11_items", "POST", 2), sid, 1, &mut w, &mut r).await;
        assert!(matches!(res, DbResponse::Batch { .. }), "post-backup write");

        let _ = w.shutdown().await;
        let mut tmp = [0u8; 1];
        let _ = r.read(&mut tmp).await;
        handle.shutdown().await;
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // ---- Restore: point at the boot #1 snapshot, via the real restore()
    // entry point (not a hand-rolled copy) ----
    let parent = data_dir.parent().unwrap().to_path_buf();

    let restore_report =
        restore::restore(&report.dest_dir, &data_dir, false).expect("restore succeeds");
    assert!(restore_report.files_restored > 0);
    assert_eq!(
        restore_report.users_invalidated, 1,
        "the single admin account's tickets must be invalidated"
    );
    let pre_restore_backup = restore_report
        .pre_restore_backup
        .clone()
        .expect("a pre-existing data_dir must be preserved as a sibling");
    assert!(
        pre_restore_backup.exists(),
        ".pre_restore_backup_* sibling must exist on disk"
    );

    // Independently confirm the sibling matches the expected naming
    // convention (`.pre_restore_backup_<timestamp>`) by scanning the
    // parent directory — not just trusting the report's own field.
    let found_pre_restore_sibling = std::fs::read_dir(&parent)
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| {
            e.file_name()
                .to_string_lossy()
                .contains(".pre_restore_backup_")
        });
    assert!(
        found_pre_restore_sibling,
        "a .pre_restore_backup_* sibling directory must be discoverable under {}",
        parent.display()
    );

    // ---- Boot #3: against the now-restored data_dir ----
    let handle = launch(data_dir.clone(), port, password).await;
    let addr = handle.first_tls_exporter_addr().unwrap();
    assert_eq!(addr.port(), port, "third boot reuses same port");

    let (mut r, mut w, sid) = login(addr, password).await;
    let res = roundtrip(&read_req("ri11_items"), sid, 1, &mut w, &mut r).await;
    match res {
        DbResponse::Batch { response } => {
            let rd = response
                .results
                .get("rd")
                .expect("rd alias must exist after restore");
            assert_eq!(
                rd.records.len(),
                1,
                "only the PRE-backup row must survive restore — the POST-backup \
                 write must be rolled back"
            );
            assert_eq!(rd.records[0].get_value_str("sku"), Some("PRE"));
            assert_eq!(rd.records[0].get_value_i64("qty"), Some(1));
        }
        DbResponse::Error { code, message } => {
            panic!("expected restored data, got error {{ code: {code:?}, message: {message:?} }}");
        }
        other => panic!("expected Batch, got {:?}", other),
    }

    // ---- The OLD ticket (captured before backup / before restore) must
    // now be REJECTED — invalidate_all_tickets ran during restore. ----
    let resumed = attempt_resume(addr, old_ticket).await;
    assert!(
        !resumed,
        "a resumption ticket issued before the restore point must be rejected \
         after restore invalidates all tickets"
    );

    let _ = w.shutdown().await;
    let mut tmp = [0u8; 1];
    let _ = r.read(&mut tmp).await;
    handle.shutdown().await;
}

// ----------------------------------------------------------------------------
// CR-B7: pre-swap ticket invalidation reordering — a failure at the
// invalidation step (now staged BEFORE the atomic swap) must leave the
// CURRENT (pre-restore) data_dir completely untouched, with no
// `.pre_restore_backup_*` sibling created at all (the swap never runs).
// ----------------------------------------------------------------------------

/// Snapshot every file's relative path + contents under `dir`, so a later
/// call can assert nothing changed at all (not just "same file count").
fn snapshot_dir_contents(dir: &std::path::Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    fn walk(
        root: &std::path::Path,
        dir: &std::path::Path,
        out: &mut std::collections::BTreeMap<String, Vec<u8>>,
    ) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, out);
            } else if path.is_file() {
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                out.insert(rel, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restore_failure_at_pre_swap_invalidation_leaves_data_dir_untouched() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let password: &[u8] = b"correct horse battery staple CR-B7";

    let data_temp = TempDir::new().unwrap();
    let data_dir = data_temp.path().to_path_buf();

    // ---- Boot #1: create table, write a row, then shut down cleanly. ----
    {
        let handle = launch(data_dir.clone(), 0, password).await;
        let addr = handle.first_tls_exporter_addr().unwrap();
        let (mut r, mut w, sid) = login(addr, password).await;

        let res = roundtrip(&create_table_req("crb7_items"), sid, 1, &mut w, &mut r).await;
        assert!(matches!(res, DbResponse::Batch { .. }), "create_table");
        let res = roundtrip(&write_req("crb7_items", "ORIG", 7), sid, 2, &mut w, &mut r).await;
        assert!(matches!(res, DbResponse::Batch { .. }), "write");

        let _ = w.shutdown().await;
        let mut tmp = [0u8; 1];
        let _ = r.read(&mut tmp).await;
        handle.shutdown().await;
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // Snapshot the CURRENT (pre-restore) data_dir contents in full, so we
    // can later assert byte-for-byte that nothing changed.
    let pre_restore_snapshot = snapshot_dir_contents(&data_dir);
    assert!(
        !pre_restore_snapshot.is_empty(),
        "sanity: data_dir must contain files before the failed restore attempt"
    );

    // ---- Backup: snapshot the current state ----
    let backup_dest = TempDir::new().unwrap();
    let report = backup::backup(&data_dir, backup_dest.path()).expect("backup ok");

    // ---- Corrupt the SNAPSHOT's `users` store so the pre-swap
    // invalidation step (which opens `temp_dir/users`, a COPY of this
    // snapshot's `users`) fails structurally, before the swap ever runs.
    // The corrupted bytes' manifest entries are patched IN PLACE (same
    // relative path, freshly-computed sha256/size for the corrupted
    // content) so `verify_manifest` (Step 2, which runs BEFORE the copy)
    // still passes — the failure this test targets must occur at the NEW
    // pre-swap invalidation step (Step 4), not at manifest verification. ----
    let snapshot_users_dir = report.dest_dir.join("users");
    assert!(
        snapshot_users_dir.is_dir(),
        "sanity: snapshot must contain a users/ directory to corrupt"
    );
    let corrupted_bytes: &[u8] = b"CR-B7 deliberately corrupted, not a valid fjall file";
    let mut corrupted_rel_paths: Vec<String> = Vec::new();
    fn corrupt_recursive(
        root: &std::path::Path,
        dir: &std::path::Path,
        corrupted_bytes: &[u8],
        corrupted_rel_paths: &mut Vec<String>,
    ) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                corrupt_recursive(root, &path, corrupted_bytes, corrupted_rel_paths);
            } else if path.is_file() {
                std::fs::write(&path, corrupted_bytes).unwrap();
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                corrupted_rel_paths.push(rel);
            }
        }
    }
    corrupt_recursive(
        &report.dest_dir,
        &snapshot_users_dir,
        corrupted_bytes,
        &mut corrupted_rel_paths,
    );
    assert!(
        !corrupted_rel_paths.is_empty(),
        "sanity: at least one users/ file must have been corrupted"
    );

    let manifest_path = report.dest_dir.join(backup::MANIFEST_FILE_NAME);
    let manifest_raw = std::fs::read(&manifest_path).unwrap();
    let mut manifest: backup::Manifest = serde_json::from_slice(&manifest_raw).unwrap();
    let corrupted_sha256 = hex::encode(shamir_connect::common::crypto::sha256(corrupted_bytes));
    let corrupted_size = corrupted_bytes.len() as u64;
    for entry in &mut manifest.files {
        if corrupted_rel_paths.contains(&entry.path) {
            entry.sha256 = corrupted_sha256.clone();
            entry.size_bytes = corrupted_size;
        }
    }
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();

    // Sanity: the patched manifest verifies cleanly against the corrupted
    // (but now self-consistent) snapshot — otherwise this test would be
    // exercising manifest verification (Step 2), not the pre-swap
    // invalidation step (Step 4) it's meant to target.
    backup::verify_manifest(&report.dest_dir)
        .expect("patched manifest must verify against the corrupted snapshot content");

    // ---- Restore MUST fail at the pre-swap invalidation step (the
    // corrupted `users` store cannot be opened as a fjall keyspace). Uses
    // `force: true` to skip the Step 1 liveness probe, which otherwise
    // legitimately mutates the LIVE data_dir's own server_meta
    // (`touch_last_started_at` + fjall's own journal-truncation-on-open) —
    // that mutation is pre-existing Step-1 behaviour, unrelated to this
    // task's reordering fix, and would otherwise pollute the
    // byte-for-byte comparison below with an unrelated diff. ----
    let err = restore::restore(&report.dest_dir, &data_dir, true).unwrap_err();
    eprintln!("restore failed as expected: {err}");

    // ---- Core guarantee: data_dir is BYTE-FOR-BYTE unchanged, and no
    // `.pre_restore_backup_*` sibling was created (the swap never ran). ----
    let post_failure_snapshot = snapshot_dir_contents(&data_dir);
    if pre_restore_snapshot != post_failure_snapshot {
        let mut diffs = Vec::new();
        let all_keys: std::collections::BTreeSet<&String> = pre_restore_snapshot
            .keys()
            .chain(post_failure_snapshot.keys())
            .collect();
        for key in all_keys {
            let before = pre_restore_snapshot.get(key);
            let after = post_failure_snapshot.get(key);
            if before != after {
                diffs.push(format!(
                    "{key}: before={:?} bytes, after={:?} bytes",
                    before.map(|v| v.len()),
                    after.map(|v| v.len())
                ));
            }
        }
        panic!(
            "data_dir must be completely untouched after a failed restore \
             (pre-swap invalidation failure must abort before the atomic swap); \
             differing entries: {diffs:#?}"
        );
    }

    // Scoped to a sibling name derived from THIS test's own data_dir name
    // (`restore.rs`'s naming scheme is `{dir_name}.pre_restore_backup_{stamp}`)
    // — `parent` is the shared OS temp root, so a plain
    // `.pre_restore_backup_` substring scan would also match siblings
    // legitimately created by OTHER tests running concurrently in this
    // same file (e.g. `restore_cli_rolls_back_data_and_invalidates_tickets`).
    let parent = data_dir.parent().unwrap().to_path_buf();
    let data_dir_name = data_dir.file_name().unwrap().to_string_lossy().into_owned();
    let sibling_prefix = format!("{data_dir_name}.pre_restore_backup_");
    let found_pre_restore_sibling = std::fs::read_dir(&parent)
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| e.file_name().to_string_lossy().starts_with(&sibling_prefix));
    assert!(
        !found_pre_restore_sibling,
        "no .pre_restore_backup_* sibling must exist for this test's data_dir — the swap never ran"
    );

    // ---- Regression: the server can still boot normally against the
    // untouched, original data_dir. ----
    let handle = launch(data_dir.clone(), 0, password).await;
    let addr = handle.first_tls_exporter_addr().unwrap();
    let (mut r, mut w, sid) = login(addr, password).await;
    let res = roundtrip(&read_req("crb7_items"), sid, 1, &mut w, &mut r).await;
    match res {
        DbResponse::Batch { response } => {
            let rd = response.results.get("rd").expect("rd alias must exist");
            assert_eq!(rd.records.len(), 1, "original data must still be intact");
            assert_eq!(rd.records[0].get_value_str("sku"), Some("ORIG"));
        }
        other => panic!(
            "expected Batch after untouched data_dir reboot, got {:?}",
            other
        ),
    }
    let _ = w.shutdown().await;
    let mut tmp = [0u8; 1];
    let _ = r.read(&mut tmp).await;
    handle.shutdown().await;
}
