//! End-to-end permission enforcement tests (Shomer access fabric).
//!
//! Exercises the full wire path: SCRAM-authenticated session →
//! `ShamirDbHandler::execute` → `ShamirDb::execute_as` → `authorize_access`.
//!
//! Scenarios:
//!   1. **deny/allow by mode** — admin creates table, chmod 0o700 (owner-only);
//!      non-admin user is DENIED; admin is ALLOWED.
//!   2. **group grant** — admin creates group, adds user, chgrp + chmod group-read;
//!      user is now ALLOWED via group; non-member is DENIED.
//!   3. **open default** — new resources default to 0o777; any authenticated
//!      user is ALLOWED.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
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
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ProfileKind, SecurityConfig,
    TlsConfig,
};
use shamir_server::db_handler::{DbRequest, DbResponse};
use shamir_server::server::{BootstrapMode, ServerLauncher};
use shamir_server::version::CURRENT_QUERY_LANG_VERSION;

// --------------------------------------------------------------------------
// Wire-frame mirrors (same as mvp_e2e.rs)
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
    #[serde(default)]
    server_query_version: u8,
}

// --------------------------------------------------------------------------
// Helpers
// --------------------------------------------------------------------------

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
        security: SecurityConfig {
            auth_init_rate_per_second: 1000,
            ..Default::default()
        },
        audit: Default::default(),
        observability: shamir_server::config::ObservabilityConfig {
            addr: String::new(),
        },
    }
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
        std::time::Duration::from_secs(10),
        read_frame(r, MAX_FRAME_SIZE_DEFAULT),
    )
    .await
    .expect("response within 10s")
    .expect("read response");
    let resp_envelope = ResponseEnvelope::from_msgpack(&resp_bytes).expect("response envelope");
    assert_eq!(resp_envelope.request_id, Some(rid), "request_id echoed");
    rmp_serde::from_slice(&resp_envelope.res).expect("decode DbResponse")
}

/// SCRAM-authenticate and return (session_id, read_half, write_half, next_rid).
async fn scram_login(
    server_addr: std::net::SocketAddr,
    username: &str,
    password: &[u8],
) -> (
    [u8; 32],
    tokio::io::ReadHalf<tokio_rustls::client::TlsStream<TcpStream>>,
    tokio::io::WriteHalf<tokio_rustls::client::TlsStream<TcpStream>>,
    u32,
) {
    let norm_username = NormalizedUsername::from_raw(username).expect("username");

    let client_cfg = make_client_config_no_ca();
    let connector = TlsConnector::from(client_cfg);
    let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let tcp = match TcpStream::connect(server_addr).await {
        Ok(t) => t,
        Err(e) => panic!("TCP connect to {} failed: {}", server_addr, e),
    };
    let tls = match connector.connect(server_name, tcp).await {
        Ok(t) => t,
        Err(e) => panic!("TLS handshake to {} failed: {}", server_addr, e),
    };
    let exporter = extract_tls_exporter(&tls).expect("client exporter");

    let hs = HandshakeBuilder::new(norm_username, TransportKind::Tcp, BindingMode::TlsExporter)
        .tls_exporter(exporter)
        .accept_new_host(true)
        .build()
        .expect("handshake builder");

    let (mut r, mut w) = split(tls);

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

    // derive proof
    let mut password_buf = password.to_vec();
    let (proof, derived, am) = hs
        .process_challenge(&challenge, &mut password_buf)
        .expect("process challenge");

    // send proof
    let proof_wire = WireClientProof {
        client_proof: proof.to_vec(),
    };
    write_frame(&mut w, &rmp_serde::to_vec(&proof_wire).unwrap())
        .await
        .expect("send proof");

    // auth_ok
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

    hs.process_auth_ok(&auth_ok, &derived, &am, |_pin| {})
        .expect("process auth_ok");

    (session_id, r, w, 1)
}

/// Build a simple `BatchRequest` that reads from a table.
fn read_batch(table: &str) -> shamir_db::query::batch::BatchRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("read");
    b.query("rd", shamir_query_builder::Query::from(table));
    b.build()
}

/// Build a batch with a single chmod op on a table.
fn chmod_batch(db: &str, store: &str, table: &str, mode: u16) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("chmod");
    b.chmod(
        "cm",
        shamir_query_builder::ddl::chmod(
            shamir_query_builder::ddl::res::table(db, store, table),
            mode,
        ),
    );
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db.into(),
        batch: b.build(),
    }
}

/// Build a batch with a single chmod op on a database (top-level container).
fn chmod_db_batch(db: &str, mode: u16) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("chmod_db");
    b.chmod(
        "cm",
        shamir_query_builder::ddl::chmod(shamir_query_builder::ddl::res::database(db), mode),
    );
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db.into(),
        batch: b.build(),
    }
}

/// Build a batch with a single chmod op on a store/repo.
fn chmod_store_batch(db: &str, store: &str, mode: u16) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("chmod_store");
    b.chmod(
        "cm",
        shamir_query_builder::ddl::chmod(shamir_query_builder::ddl::res::store(db, store), mode),
    );
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db.into(),
        batch: b.build(),
    }
}

/// Build a batch with a single chgrp op on a table.
fn chgrp_batch(db: &str, store: &str, table: &str, group: u64) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("chgrp");
    b.chgrp(
        "cg",
        shamir_query_builder::ddl::chgrp(
            shamir_query_builder::ddl::res::table(db, store, table),
            Some(group),
        ),
    );
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db.into(),
        batch: b.build(),
    }
}

/// Build a batch creating a group.
fn create_group_req(db: &str, name: &str) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("mkgrp");
    b.create_group("mk", shamir_query_builder::ddl::create_group(name));
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db.into(),
        batch: b.build(),
    }
}

/// Build a batch adding a user to a group.
fn add_group_member_req(db: &str, group_name: &str, user_id: u64) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("addmember");
    b.add_group_member(
        "am",
        shamir_query_builder::ddl::add_group_member(
            shamir_query_builder::ddl::GroupRef::Name {
                name: group_name.to_string(),
            },
            user_id,
        ),
    );
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db.into(),
        batch: b.build(),
    }
}

/// Build a create_db request.
fn create_db_req(db_name: &str) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("mkdb");
    b.create_db("mk", shamir_query_builder::ddl::create_db(db_name));
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: b.build(),
    }
}

/// Build a create_repo + create_table request.
fn create_repo_table_req(db_name: &str, table_name: &str) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("setup");
    b.create_repo("mr", shamir_query_builder::ddl::create_repo("main"));
    b.create_table("tb", shamir_query_builder::ddl::create_table(table_name));
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db_name.into(),
        batch: b.build(),
    }
}

/// Build a read request against a table.
fn read_req(db_name: &str, table_name: &str) -> DbRequest {
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db_name.into(),
        batch: read_batch(table_name),
    }
}

/// Compute the stable principal id for a username (mirrors Session::principal_id).
fn principal_id(username: &str) -> u64 {
    // Must match Session::principal_id (masked to 63 bits so the id fits i64).
    fxhash::hash64(username) & (i64::MAX as u64)
}

async fn cleanup(w: &mut (impl AsyncWriteExt + Unpin), r: &mut (impl AsyncReadExt + Unpin)) {
    let _ = w.shutdown().await;
    let mut tmp = [0u8; 1];
    let _ = r.read(&mut tmp).await;
}

// --------------------------------------------------------------------------
// Tests
// --------------------------------------------------------------------------

/// Scenario 1: deny non-owner, allow owner when mode is 0o700.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn permission_deny_allow_by_mode() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().expect("tempdir");
    let config = make_test_config(&temp);

    let admin_pw = b"admin-password".to_vec();
    let bootstrap = BootstrapMode::Password {
        username: "admin".into(),
        password: Zeroizing::new(admin_pw.clone()),
    };

    let launcher = ServerLauncher { config, bootstrap };
    let handle = launcher.launch().await.expect("launcher boot");
    let server_addr = handle.first_tls_exporter_addr().expect("bound address");

    // --- Admin login ---
    let (admin_sid, mut admin_r, mut admin_w, mut admin_rid) =
        scram_login(server_addr, "admin", &admin_pw).await;

    // --- Admin: create db + repo + table ---
    let db_name = "ptest";
    let table_name = "secret";

    let resp = roundtrip(
        &create_db_req(db_name),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "create_db: {:?}",
        resp
    );

    let resp = roundtrip(
        &create_repo_table_req(db_name, table_name),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "setup: {:?}",
        resp
    );

    // --- Admin: chmod table to 0o700 (owner-only rwx) ---
    let resp = roundtrip(
        &chmod_batch(db_name, "main", table_name, 0o700),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "chmod: {:?}",
        resp
    );

    // --- Create a non-admin user ---
    let user_pw = b"user-password".to_vec();
    let resp = roundtrip(
        &DbRequest::CreateScramUser {
            name: "alice".into(),
            password: String::from_utf8(user_pw.clone()).expect("utf8"),
            roles: vec![],
        },
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::UserCreated { .. }),
        "create user: {:?}",
        resp
    );

    // --- Alice logs in ---
    let (alice_sid, mut alice_r, mut alice_w, mut alice_rid) =
        scram_login(server_addr, "alice", &user_pw).await;

    // --- Alice tries to read from secret table → DENIED ---
    let resp = roundtrip(
        &read_req(db_name, table_name),
        alice_sid,
        &mut alice_rid,
        &mut alice_w,
        &mut alice_r,
    )
    .await;
    match resp {
        DbResponse::Error { code, message } => {
            assert_eq!(
                code, "access_denied",
                "alice should be denied, got code={}, msg={}",
                code, message
            );
            assert!(
                message.contains("access denied"),
                "message should describe denial: {}",
                message
            );
        }
        other => panic!("expected access_denied error, got {:?}", other),
    }

    // --- Admin reads from the same table → ALLOWED ---
    let resp = roundtrip(
        &read_req(db_name, table_name),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "admin should be allowed: {:?}",
        resp
    );

    // Cleanup
    cleanup(&mut alice_w, &mut alice_r).await;
    cleanup(&mut admin_w, &mut admin_r).await;
    handle.shutdown().await;
}

/// Scenario 2: group grant — user allowed via group, non-member denied.
///
/// Three SCRAM connections from the same loopback subnet (admin + bob +
/// carol); the test config sets `auth_init_rate_per_second = 1000` so the
/// §8.6 warmup rate (/4 = 250/sec) does not throttle multi-login.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn permission_group_grant() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().expect("tempdir");
    let config = make_test_config(&temp);

    let admin_pw = b"admin-password".to_vec();
    let bootstrap = BootstrapMode::Password {
        username: "admin".into(),
        password: Zeroizing::new(admin_pw.clone()),
    };

    let launcher = ServerLauncher { config, bootstrap };
    let handle = match launcher.launch().await {
        Ok(h) => h,
        Err(e) => panic!("launcher boot failed: {e}"),
    };
    let server_addr = handle.first_tls_exporter_addr().expect("bound address");
    let (admin_sid, mut admin_r, mut admin_w, mut admin_rid) =
        scram_login(server_addr, "admin", &admin_pw).await;

    let db_name = "gtest";
    let table_name = "shared";

    let resp = roundtrip(
        &create_db_req(db_name),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "create_db: {:?}",
        resp
    );

    let resp = roundtrip(
        &create_repo_table_req(db_name, table_name),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "setup: {:?}",
        resp
    );

    // G.4c: new objects default to enforced (0o700, owner=System since admin
    // is superuser). The group-grant test must let a group member traverse the
    // db + repo ancestors to reach the table (whose group bits are the SUBJECT
    // of this test). Open the db + repo so Execute is granted to everyone.
    let resp = roundtrip(
        &chmod_db_batch(db_name, 0o755),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "chmod_db: {:?}",
        resp
    );
    let resp = roundtrip(
        &chmod_store_batch(db_name, "main", 0o755),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "chmod_store: {:?}",
        resp
    );

    // Create two non-admin users: bob (member) and carol (non-member)
    let bob_pw = b"bob-password".to_vec();
    let carol_pw = b"carol-password".to_vec();

    for (name, pw) in [("bob", &bob_pw), ("carol", &carol_pw)] {
        let resp = roundtrip(
            &DbRequest::CreateScramUser {
                name: name.into(),
                password: String::from_utf8(pw.clone()).expect("utf8"),
                roles: vec![],
            },
            admin_sid,
            &mut admin_rid,
            &mut admin_w,
            &mut admin_r,
        )
        .await;
        assert!(
            matches!(resp, DbResponse::UserCreated { .. }),
            "create {}: {:?}",
            name,
            resp
        );
    }

    // --- Admin: create group ---
    let resp = roundtrip(
        &create_group_req(db_name, "devs"),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "create_group: {:?}",
        resp
    );

    // Extract the group_id from the response.
    let group_id: u64 = match &resp {
        DbResponse::Batch { response } => response
            .results
            .get("mk")
            .and_then(|r| r.records.first())
            .and_then(|r| r.get_value_u64("group_id"))
            .expect("group_id in response"),
        _ => panic!("unexpected"),
    };

    // Add bob to the group (use principal_id = fxhash of username)
    let bob_pid = principal_id("bob");
    let resp = roundtrip(
        &add_group_member_req(db_name, "devs", bob_pid),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "add_member: {:?}",
        resp
    );

    // chgrp the table to the group
    let resp = roundtrip(
        &chgrp_batch(db_name, "main", table_name, group_id),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "chgrp: {:?}",
        resp
    );

    // chmod: owner rwx + group r-x + other --- (0o750)
    let resp = roundtrip(
        &chmod_batch(db_name, "main", table_name, 0o750),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "chmod: {:?}",
        resp
    );

    // --- Bob (group member) reads → ALLOWED ---
    let (bob_sid, mut bob_r, mut bob_w, mut bob_rid) =
        scram_login(server_addr, "bob", &bob_pw).await;

    let resp = roundtrip(
        &read_req(db_name, table_name),
        bob_sid,
        &mut bob_rid,
        &mut bob_w,
        &mut bob_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "bob (group member) should be allowed: {:?}",
        resp
    );

    // --- Carol (non-member) reads → DENIED ---
    let (carol_sid, mut carol_r, mut carol_w, mut carol_rid) =
        scram_login(server_addr, "carol", &carol_pw).await;

    let resp = roundtrip(
        &read_req(db_name, table_name),
        carol_sid,
        &mut carol_rid,
        &mut carol_w,
        &mut carol_r,
    )
    .await;
    match resp {
        DbResponse::Error { code, message } => {
            assert_eq!(
                code, "access_denied",
                "carol should be denied, got code={}, msg={}",
                code, message
            );
        }
        other => panic!("expected access_denied for carol, got {:?}", other),
    }

    // Cleanup
    cleanup(&mut bob_w, &mut bob_r).await;
    cleanup(&mut carol_w, &mut carol_r).await;
    cleanup(&mut admin_w, &mut admin_r).await;
    handle.shutdown().await;
}

/// Scenario 3: enforced default denies a stranger; explicit OPEN (0o777)
/// allows any authenticated user.
///
/// G.4c (Strategy A): new objects default to enforced owner-rwx (0o700), so a
/// non-owner stranger is now DENIED by default. This test proves BOTH paths:
///   (1) the enforced default denies a stranger (no chmod);
///   (2) after an explicit chmod to OPEN (0o777) on db + repo + table, any
///       authenticated user is ALLOWED (the open-mode path is unchanged).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn permission_open_default_allows_any_user() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().expect("tempdir");
    let config = make_test_config(&temp);

    let admin_pw = b"admin-password".to_vec();
    let bootstrap = BootstrapMode::Password {
        username: "admin".into(),
        password: Zeroizing::new(admin_pw.clone()),
    };

    let launcher = ServerLauncher { config, bootstrap };
    let handle = launcher.launch().await.expect("launcher boot");
    let server_addr = handle.first_tls_exporter_addr().expect("bound address");

    // --- Admin login ---
    let (admin_sid, mut admin_r, mut admin_w, mut admin_rid) =
        scram_login(server_addr, "admin", &admin_pw).await;

    let db_name = "opentest";
    let table_name = "public";

    let resp = roundtrip(
        &create_db_req(db_name),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "create_db: {:?}",
        resp
    );

    // No chmod on the table yet — G.4c: create defaults are enforced (0o700).
    let resp = roundtrip(
        &create_repo_table_req(db_name, table_name),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "setup: {:?}",
        resp
    );

    // Create a regular user
    let dave_pw = b"dave-password".to_vec();
    let resp = roundtrip(
        &DbRequest::CreateScramUser {
            name: "dave".into(),
            password: String::from_utf8(dave_pw.clone()).expect("utf8"),
            roles: vec![],
        },
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::UserCreated { .. }),
        "create dave: {:?}",
        resp
    );

    // --- Dave reads from public table → DENIED (enforced default 0o700) ---
    let (dave_sid, mut dave_r, mut dave_w, mut dave_rid) =
        scram_login(server_addr, "dave", &dave_pw).await;

    let resp = roundtrip(
        &read_req(db_name, table_name),
        dave_sid,
        &mut dave_rid,
        &mut dave_w,
        &mut dave_r,
    )
    .await;
    match resp {
        DbResponse::Error { code, .. } => assert_eq!(
            code, "access_denied",
            "enforced default (0o700) should deny a non-owner stranger"
        ),
        other => panic!(
            "expected access_denied under enforced default, got {:?}",
            other
        ),
    }

    // --- Admin chmod db + repo + table to OPEN (0o777) ---
    for req in [
        chmod_db_batch(db_name, 0o777),
        chmod_store_batch(db_name, "main", 0o777),
        chmod_batch(db_name, "main", table_name, 0o777),
    ] {
        let resp = roundtrip(&req, admin_sid, &mut admin_rid, &mut admin_w, &mut admin_r).await;
        assert!(
            matches!(resp, DbResponse::Batch { .. }),
            "chmod open: {:?}",
            resp
        );
    }

    // --- Dave reads again → ALLOWED (explicit OPEN path is unchanged) ---
    let resp = roundtrip(
        &read_req(db_name, table_name),
        dave_sid,
        &mut dave_rid,
        &mut dave_w,
        &mut dave_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "dave should be allowed after explicit chmod 0o777: {:?}",
        resp
    );

    // Cleanup
    cleanup(&mut dave_w, &mut dave_r).await;
    cleanup(&mut admin_w, &mut admin_r).await;
    handle.shutdown().await;
}
