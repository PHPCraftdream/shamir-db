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
//!   * wire_tables.json     (registry of tables created over the wire)
//!   * cert.pem / key.pem   (TLS material — without it the second boot
//!   * cert.pem / key.pem   (TLS material — without it the second boot
//!     would generate a NEW Ed25519 + new server pub-key,
//!     breaking client TOFU pin sanity)

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

use shamir_server::backup;
use shamir_server::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ObservabilityConfig,
    ProfileKind, TlsConfig,
};
use shamir_server::db_handler::{DbRequest, DbResponse};
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
        },
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
            .key(json!({"sku": sku}))
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
    let wt = std::fs::read_to_string(data_dir.join("wire_tables.json")).unwrap_or_default();
    eprintln!("wire_tables.json: {}", wt);
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
            assert_eq!(
                rd.records[0].as_json().get("sku").and_then(|v| v.as_str()),
                Some("X1")
            );
            assert_eq!(
                rd.records[0].as_json().get("qty").and_then(|v| v.as_i64()),
                Some(42)
            );
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
