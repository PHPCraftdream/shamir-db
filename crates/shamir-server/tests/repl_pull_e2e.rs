//! R0-c — end-to-end replication pull over loopback TLS + SCRAM.
//!
//! Spins up a real `ServerLauncher` against a `TempDir`, then drives the
//! full wire path (TLS 1.3 + SCRAM-Argon2id + `RequestEnvelope`/
//! `ResponseEnvelope`) to prove that `DbRequest::Repl` travels over the
//! real protocol and comes back as `DbResponse::Repl`.
//!
//! Scenarios (REPLICATION §5):
//!   A. **Deny-by-default** — a SCRAM user WITHOUT the `replicator` role
//!      sends `ReplHello` and is rejected with `code == "bad_role"`. This
//!      is the security-critical core: the role gate is enforced on the
//!      real wire path, not only in unit tests.
//!   B. **Happy-path** — `ReplHello` advertises `app/main` with
//!      `current_version > 0`; `ReplPull` returns encoded changelog
//!      events emitted by real upserts.
//!   C. **Long-poll** — `ReplPull` on an empty tail with `wait_ms: Some(200)`
//!      returns promptly (does not hang).
//!
//! Access path: admin `chmod`s db + repo + table to `0o777` (the explicit
//! OPEN pattern proven by `permission_e2e.rs` Scenario 3) so the
//! `replicator`-role user gains per-repo `Read` via the normal Shomer DAC
//! path — NOT a superuser-bypass fallback. Per-repo authz (the
//! `denied_repo` branch in `repl_handler`) is covered by the R0-b unit
//! tests; this e2e focuses on the wire-level `Hello`/`Pull` path + the
//! role gate.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;
use tempfile::TempDir;
use tokio::io::{split, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use shamir_connect::client::handshake::{HandshakeBuilder, ServerAuthOk, ServerChallenge};
use shamir_connect::common::envelope::{RequestEnvelope, ResponseEnvelope};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::common::username::NormalizedUsername;

use shamir_transport_tcp::framing::{read_frame, write_frame, MAX_FRAME_SIZE_DEFAULT};
use shamir_transport_tcp::tls::{extract_tls_exporter, make_client_config_no_ca};

use shamir_query_types::wire::repl::{ReplRequest, ReplResponse};

use shamir_server::db_handler::{DbRequest, DbResponse};
use shamir_server::version::CURRENT_QUERY_LANG_VERSION;

mod common;

// ---------------------------------------------------------------------------
// Wire-frame mirrors of the auth_init / challenge / client_proof / auth_ok
// envelopes. Cargo gives every `tests/*.rs` its own crate, so each file
// keeps a local copy of these structs (mirrors `mvp_e2e.rs` /
// `permission_e2e.rs`).
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
    expires_at_ns: u64,
    #[serde(default, with = "serde_bytes")]
    resumption_ticket: Vec<u8>,
    #[serde(default)]
    resumption_expires_at_ns: u64,
    #[serde(default)]
    server_query_version: u8,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Send `req`, read back the decoded `DbResponse`, echoing `request_id`.
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

type HalfStream = tokio_rustls::client::TlsStream<TcpStream>;

/// Full TLS + SCRAM-Argon2id login. Returns `(session_id, read, write, next_rid)`.
async fn scram_login(
    server_addr: std::net::SocketAddr,
    username: &str,
    password: &[u8],
) -> (
    [u8; 32],
    tokio::io::ReadHalf<HalfStream>,
    tokio::io::WriteHalf<HalfStream>,
    u32,
) {
    let norm_username = NormalizedUsername::from_raw(username).expect("username");

    let client_cfg = make_client_config_no_ca();
    let connector = TlsConnector::from(client_cfg);
    let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let tcp = TcpStream::connect(server_addr).await.expect("tcp connect");
    let tls = connector
        .connect(server_name, tcp)
        .await
        .expect("tls handshake");
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

    // derive + send proof
    let mut password_buf = password.to_vec();
    let (proof, derived, am) = hs
        .process_challenge(&challenge, &mut password_buf)
        .expect("process challenge");
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

/// Graceful socket teardown.
async fn cleanup(w: &mut (impl AsyncWriteExt + Unpin), r: &mut (impl AsyncReadExt + Unpin)) {
    let _ = w.shutdown().await;
    let mut tmp = [0u8; 1];
    let _ = r.read(&mut tmp).await;
}

/// Build a `create_db` request issued against `default`.
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

/// Build a `create_repo` + `create_table` request.
fn create_repo_table_req(db_name: &str, repo: &str, table: &str) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("setup");
    b.create_repo("mr", shamir_query_builder::ddl::create_repo(repo));
    b.create_table("tb", shamir_query_builder::ddl::create_table(table));
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db_name.into(),
        batch: b.build(),
    }
}

/// `chmod` a db / repo / table to `mode` — used to OPEN (0o777) the
/// access path for the non-superuser `replicator` session. `session_id`
/// is the signing session's bearer token — chmod is HMAC-gated (task #542).
fn chmod_req(
    session_id: [u8; 32],
    db: &str,
    repo: Option<&str>,
    table: Option<&str>,
    mode: u16,
) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("chmod");
    let res = match (repo, table) {
        (Some(r), Some(t)) => shamir_query_builder::ddl::res::table(db, r, t),
        (Some(r), None) => shamir_query_builder::ddl::res::store(db, r),
        _ => shamir_query_builder::ddl::res::database(db),
    };
    let key = shamir_query_types::hmac::derive_session_hmac_key(&session_id);
    let tag = shamir_query_types::hmac::compute_tag_hex(
        &key,
        &shamir_query_types::hmac::canonical_chmod(&res, mode),
    );
    b.chmod("cm", shamir_query_builder::ddl::chmod(res, mode).hmac(tag));
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db.into(),
        batch: b.build(),
    }
}

/// `ReplHello` request.
fn repl_hello(node_id: &str) -> DbRequest {
    DbRequest::Repl(ReplRequest::Hello {
        proto_ver: 1,
        node_id: node_id.into(),
    })
}

/// `ReplPull` request.
fn repl_pull(
    db: &str,
    repo: &str,
    from_version: u64,
    limit: u32,
    wait_ms: Option<u32>,
) -> DbRequest {
    DbRequest::Repl(ReplRequest::Pull {
        db: db.into(),
        repo: repo.into(),
        from_version,
        limit,
        wait_ms,
    })
}

// ---------------------------------------------------------------------------
// Scenario A — deny-by-default over the wire.
// ---------------------------------------------------------------------------

/// A SCRAM user WITHOUT the `replicator` role sends `ReplHello` over a
/// real TLS+SCRAM session and is rejected with `code == "bad_role"`.
///
/// This is the security-critical core of R0-c: the role gate fires on the
/// live wire path (not just in unit tests). The request reaches the
/// handler via `DbRequest::Repl`, is dispatched into `handle_repl`, and
/// the `bad_role` reply travels back as `DbResponse::Repl(ReplResponse::Error)`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repl_deny_by_default_no_replicator_role() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().expect("tempdir");
    let admin_pw = b"admin-password".to_vec();
    let handle = common::spawn_ephemeral(&temp, &admin_pw).await;
    let server_addr = handle.first_tls_exporter_addr().expect("bound address");

    // --- Admin: create a plain user with NO roles. ---
    let (admin_sid, mut admin_r, mut admin_w, mut admin_rid) =
        scram_login(server_addr, "admin", &admin_pw).await;

    let plain_pw = b"plain-password".to_vec();
    let plain_roles: Vec<String> = vec![];
    let plain_tag = shamir_query_types::hmac::compute_tag_hex(
        &shamir_query_types::hmac::derive_session_hmac_key(&admin_sid),
        &shamir_query_types::hmac::canonical_create_scram_user("plain", &plain_roles),
    );
    let resp = roundtrip(
        &DbRequest::CreateScramUser {
            name: "plain".into(),
            password: String::from_utf8(plain_pw.clone()).expect("utf8"),
            roles: plain_roles,
            hmac: Some(plain_tag),
        },
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::UserCreated { .. }),
        "create plain user: {:?}",
        resp,
    );

    // --- plain: TLS+SCRAM login, then ReplHello → bad_role. ---
    let (plain_sid, mut plain_r, mut plain_w, mut plain_rid) =
        scram_login(server_addr, "plain", &plain_pw).await;

    let resp = roundtrip(
        &repl_hello("n1"),
        plain_sid,
        &mut plain_rid,
        &mut plain_w,
        &mut plain_r,
    )
    .await;

    match resp {
        DbResponse::Repl(ReplResponse::Error {
            leader_epoch,
            code,
            message,
        }) => {
            assert_eq!(leader_epoch, 1, "default leader_epoch is 1");
            assert_eq!(code, "bad_role", "deny-by-default: {message}");
            assert!(
                message.contains("replicator"),
                "message should name the missing role: {message}",
            );
        }
        other => panic!("expected Repl(Error(bad_role)), got: {other:?}"),
    }

    cleanup(&mut plain_w, &mut plain_r).await;
    cleanup(&mut admin_w, &mut admin_r).await;
    handle.shutdown().await;
}

// ---------------------------------------------------------------------------
// Scenario B — happy-path: Hello + Pull with real changelog events.
// ---------------------------------------------------------------------------

/// Admin creates `app/main/items`, OPENs the access path (0o777 on db +
/// repo + table) so a non-superuser `replicator`-role session can read
/// it, writes rows (which emit changelog events on tx-commit), then the
/// `replicator` session:
///   1. `ReplHello` → `leader_epoch == 1`, `repos` contains `app/main`
///      with `current_version > 0`.
///   2. `ReplPull { from_version: 0 }` → encoded `Vec<ChangelogEvent>`
///      containing changes on `items`, `current_version > 0`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repl_hello_and_pull_with_events() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().expect("tempdir");
    let admin_pw = b"admin-password".to_vec();
    let handle = common::spawn_ephemeral(&temp, &admin_pw).await;
    let server_addr = handle.first_tls_exporter_addr().expect("bound address");

    let (admin_sid, mut admin_r, mut admin_w, mut admin_rid) =
        scram_login(server_addr, "admin", &admin_pw).await;

    let db = "app";
    let repo = "main";
    let table = "items";

    // --- Admin: create db + repo + table. ---
    let resp = roundtrip(
        &create_db_req(db),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "create_db: {resp:?}"
    );

    let resp = roundtrip(
        &create_repo_table_req(db, repo, table),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(matches!(resp, DbResponse::Batch { .. }), "setup: {resp:?}");

    // --- Admin: OPEN db + repo + table (0o777) so the `replicator`-role
    //     session (created below) can read via the normal Shomer DAC path.
    //     This is NOT a superuser-bypass — it is the explicit OPEN pattern
    //     proven by `permission_e2e.rs` Scenario 3. ---
    for (r, t) in [(None, None), (Some(repo), None), (Some(repo), Some(table))] {
        let resp = roundtrip(
            &chmod_req(admin_sid, db, r, t, 0o777),
            admin_sid,
            &mut admin_rid,
            &mut admin_w,
            &mut admin_r,
        )
        .await;
        assert!(
            matches!(resp, DbResponse::Batch { .. }),
            "chmod open: {resp:?}"
        );
    }

    // --- Admin: write rows (each upsert commits → emits a changelog
    //     event). The journal writer is async, so we poll a ReplPull until
    //     events land. ---
    for i in 0..3u64 {
        let sku = format!("X{i}");
        let mut wb = shamir_query_builder::batch::Batch::new();
        wb.id("write");
        wb.transactional();
        wb.upsert(
            "ins",
            shamir_query_builder::write::upsert(table)
                .key(mpack!({"sku": @(QueryValue::from(sku.clone()))}))
                .value(shamir_query_builder::doc! { "sku" => sku, "qty" => i as i64 }),
        );
        let req = DbRequest::Execute {
            query_version: CURRENT_QUERY_LANG_VERSION,
            db: db.into(),
            batch: wb.build(),
        };
        let resp = roundtrip(&req, admin_sid, &mut admin_rid, &mut admin_w, &mut admin_r).await;
        assert!(
            matches!(resp, DbResponse::Batch { .. }),
            "write {i}: {resp:?}"
        );
    }

    // --- Admin: create the `replicator`-role user. ---
    let repl_pw = b"repl-password".to_vec();
    let repl_roles = vec!["replicator".to_string()];
    let repl_tag = shamir_query_types::hmac::compute_tag_hex(
        &shamir_query_types::hmac::derive_session_hmac_key(&admin_sid),
        &shamir_query_types::hmac::canonical_create_scram_user("repl", &repl_roles),
    );
    let resp = roundtrip(
        &DbRequest::CreateScramUser {
            name: "repl".into(),
            password: String::from_utf8(repl_pw.clone()).expect("utf8"),
            roles: repl_roles,
            hmac: Some(repl_tag),
        },
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::UserCreated { .. }),
        "create repl user: {resp:?}",
    );

    // --- repl: login. ---
    let (repl_sid, mut repl_r, mut repl_w, mut repl_rid) =
        scram_login(server_addr, "repl", &repl_pw).await;

    // --- Poll for journal durability before asserting on Pull. The
    //     journal writer is a background task; give it a moment. ---
    let mut hello_repos = Vec::new();
    for attempt in 0..100 {
        let resp = roundtrip(
            &repl_hello("n1"),
            repl_sid,
            &mut repl_rid,
            &mut repl_w,
            &mut repl_r,
        )
        .await;
        match resp {
            DbResponse::Repl(ReplResponse::Hello {
                leader_epoch,
                repos,
            }) => {
                assert_eq!(leader_epoch, 1, "default leader_epoch is 1");
                hello_repos = repos;
                let main = hello_repos.iter().find(|r| r.db == db && r.repo == repo);
                if let Some(info) = main {
                    if info.current_version > 0 {
                        break;
                    }
                }
            }
            other => panic!("expected Repl(Hello), got: {other:?}"),
        }
        if attempt == 99 {
            panic!("journal did not durable-land events in time; repos: {hello_repos:?}");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // --- Hello assertions: app/main advertised with current_version > 0. ---
    let main = hello_repos
        .iter()
        .find(|r| r.db == db && r.repo == repo)
        .expect("app/main should be advertised to repl");
    assert!(
        main.current_version > 0,
        "current_version > 0 after writes, got {}",
        main.current_version,
    );

    // --- Pull from 0 → encoded Vec<ChangelogEvent> with changes on items. ---
    let resp = roundtrip(
        &repl_pull(db, repo, 0, 100, None),
        repl_sid,
        &mut repl_rid,
        &mut repl_w,
        &mut repl_r,
    )
    .await;
    let (events_bytes, current_version, gap_at) = match resp {
        DbResponse::Repl(ReplResponse::Pull {
            leader_epoch,
            events,
            gap_at,
            current_version,
        }) => {
            assert_eq!(leader_epoch, 1);
            (events, current_version, gap_at)
        }
        other => panic!("expected Repl(Pull), got: {other:?}"),
    };

    assert!(current_version > 0, "current_version > 0 after writes");
    assert!(
        gap_at.is_none(),
        "no gap expected on a fresh journal: {gap_at:?}"
    );

    let decoded: Vec<shamir_db::engine::ChangelogEvent> =
        rmp_serde::from_slice(&events_bytes).expect("events should decode");
    assert!(!decoded.is_empty(), "pull should return at least one event");
    assert!(
        decoded.iter().all(|e| e.repo == repo),
        "all events should target {repo}: {decoded:?}",
    );
    // Each write was a transactional upsert on `items` → at least one
    // RecordChange per event.
    assert!(
        decoded.iter().any(|e| !e.changes.is_empty()),
        "at least one event should carry record changes: {decoded:?}",
    );

    cleanup(&mut repl_w, &mut repl_r).await;
    cleanup(&mut admin_w, &mut admin_r).await;
    handle.shutdown().await;
}

// ---------------------------------------------------------------------------
// Scenario C — long-poll on an empty tail does not hang.
// ---------------------------------------------------------------------------

/// `ReplPull` with `from_version == current_version` (empty tail) and
/// `wait_ms: Some(200)` returns within a reasonable bound (~2s) with an
/// empty `events` vec. The strict 200ms is not asserted — what matters
/// is that the deadline-bounded loop in `handle_pull` terminates instead
/// of hanging forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repl_long_poll_empty_tail_does_not_hang() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().expect("tempdir");
    let admin_pw = b"admin-password".to_vec();
    let handle = common::spawn_ephemeral(&temp, &admin_pw).await;
    let server_addr = handle.first_tls_exporter_addr().expect("bound address");

    let (admin_sid, mut admin_r, mut admin_w, mut admin_rid) =
        scram_login(server_addr, "admin", &admin_pw).await;

    let db = "app";
    let repo = "main";
    let table = "items";

    // --- Admin: create db + repo + table + OPEN + one write (so the
    //     journal has a non-zero current_version to tail past). ---
    let resp = roundtrip(
        &create_db_req(db),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "create_db: {resp:?}"
    );

    let resp = roundtrip(
        &create_repo_table_req(db, repo, table),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(matches!(resp, DbResponse::Batch { .. }), "setup: {resp:?}");

    for (r, t) in [(None, None), (Some(repo), None), (Some(repo), Some(table))] {
        let resp = roundtrip(
            &chmod_req(admin_sid, db, r, t, 0o777),
            admin_sid,
            &mut admin_rid,
            &mut admin_w,
            &mut admin_r,
        )
        .await;
        assert!(
            matches!(resp, DbResponse::Batch { .. }),
            "chmod open: {resp:?}"
        );
    }

    let mut wb = shamir_query_builder::batch::Batch::new();
    wb.id("seed");
    wb.transactional();
    wb.upsert(
        "ins",
        shamir_query_builder::write::upsert(table)
            .key(mpack!({"sku": "S1"}))
            .value(shamir_query_builder::doc! { "sku" => "S1", "qty" => 1i64 }),
    );
    let req = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db.into(),
        batch: wb.build(),
    };
    let resp = roundtrip(&req, admin_sid, &mut admin_rid, &mut admin_w, &mut admin_r).await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "seed write: {resp:?}"
    );

    // --- Admin: create the `replicator`-role user. ---
    let repl_pw = b"repl-password".to_vec();
    let repl_roles = vec!["replicator".to_string()];
    let repl_tag = shamir_query_types::hmac::compute_tag_hex(
        &shamir_query_types::hmac::derive_session_hmac_key(&admin_sid),
        &shamir_query_types::hmac::canonical_create_scram_user("repl", &repl_roles),
    );
    let resp = roundtrip(
        &DbRequest::CreateScramUser {
            name: "repl".into(),
            password: String::from_utf8(repl_pw.clone()).expect("utf8"),
            roles: repl_roles,
            hmac: Some(repl_tag),
        },
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::UserCreated { .. }),
        "create repl user: {resp:?}",
    );

    let (repl_sid, mut repl_r, mut repl_w, mut repl_rid) =
        scram_login(server_addr, "repl", &repl_pw).await;

    // --- Long-poll an empty tail. `from_version = u64::MAX` is
    //     guaranteed past the highest possible commit version, so the
    //     journal read returns nothing and the deadline-bounded poll loop
    //     in `handle_pull` spins until `wait_ms` expires. Mirrors the
    //     R0-b unit test `long_poll_empty_tail_does_not_hang`. ---
    let start = Instant::now();
    let resp = roundtrip(
        &repl_pull(db, repo, u64::MAX, 100, Some(200)),
        repl_sid,
        &mut repl_rid,
        &mut repl_w,
        &mut repl_r,
    )
    .await;
    let elapsed = start.elapsed();

    let events_bytes = match resp {
        DbResponse::Repl(ReplResponse::Pull {
            leader_epoch,
            events,
            ..
        }) => {
            assert_eq!(leader_epoch, 1);
            events
        }
        other => panic!("expected Repl(Pull), got: {other:?}"),
    };

    let decoded: Vec<shamir_db::engine::ChangelogEvent> =
        rmp_serde::from_slice(&events_bytes).expect("events should decode");
    assert!(decoded.is_empty(), "no events expected on the empty tail");

    // The poll budget was 200ms; allow generous headroom for scheduler
    // jitter + wire RTT, but flag a real hang (> 5s).
    assert!(
        elapsed < Duration::from_secs(5),
        "long-poll should return within ~2s, took {:?}",
        elapsed,
    );

    cleanup(&mut repl_w, &mut repl_r).await;
    cleanup(&mut admin_w, &mut admin_r).await;
    handle.shutdown().await;
}
