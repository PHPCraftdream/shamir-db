//! MVP end-to-end smoke test.
//!
//! Spins up a real `ServerLauncher` against a `TempDir`, then connects a
//! `shamir-connect` client through actual TLS + SCRAM-Argon2id, sends a
//! [`DbRequest::Execute`] batch, and asserts the [`DbResponse::Batch`]
//! payload matches what the DB actually held.
//!
//! Verifies the entire MVP wiring chain in a single test:
//!   1. Launcher boots — durable stores open, bootstrap admin created.
//!   2. TLS listener bound.
//!   3. Client connects via TLS 1.3 (no CA verify; protocol pin is the
//!      Ed25519 signature inside `auth_ok`).
//!   4. SCRAM-Argon2id handshake succeeds (auth_init → challenge →
//!      client_proof → auth_ok).
//!   5. Post-handshake `RequestEnvelope { sid, req }` carrying
//!      `DbRequest::Execute` is processed by the connection orchestration
//!      and dispatched into the full Batch API.
//!   6. `ResponseEnvelope` decodes back into `DbResponse::Batch` with the
//!      expected records.
//!   7. Server shuts down cleanly.

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
use shamir_connect::common::crypto::sha256;
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
use shamir_server::server::{BootstrapMode, ServerLauncher};

// --------------------------------------------------------------------------
// Wire-frame mirrors of the auth_init / challenge / client_proof / auth_ok
// envelopes (kept transport-binding-local — same shape as the test fixture
// in `shamir-transport-tcp/tests/handshake_e2e.rs`).
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
    /// Resumption ticket — present iff the server issued one. v1 server
    /// always issues; an empty `Vec` here means the issuance failed
    /// (warned in server logs) and the client must re-auth from scratch.
    #[serde(default, with = "serde_bytes")]
    resumption_ticket: Vec<u8>,
    #[serde(default)]
    resumption_expires_at_ns: u64,
}

// --------------------------------------------------------------------------
// Helper: send DbRequest, receive DbResponse, echoing request_id.
// --------------------------------------------------------------------------

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
    let resp_envelope =
        ResponseEnvelope::from_msgpack(&resp_bytes).expect("response envelope");
    assert_eq!(resp_envelope.request_id, Some(rid), "request_id echoed");
    rmp_serde::from_slice(&resp_envelope.res).expect("decode DbResponse")
}

// --------------------------------------------------------------------------
// Test fixtures
// --------------------------------------------------------------------------

fn fast_kdf() -> KdfConfig {
    // Spec floor — fast enough for tests, real enough that the full code
    // path runs.
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
        logging: LoggingConfig { level: "warn".into(), slow_query_threshold_ms: 0 },
        kdf_defaults: fast_kdf(),
        argon2_concurrent_max: 4,
        listeners: vec![ListenerConfig {
            kind: ListenerKind::Tcp,
            // Port 0 = OS picks a free port; we read it back via
            // `handle.first_tls_exporter_addr()`.
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
    }
}

// --------------------------------------------------------------------------
// The test
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mvp_full_pipeline_tls_scram_batch_query() {
    // Install rustls crypto provider once (idempotent).
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().expect("tempdir");
    let config = make_test_config(&temp);

    // Bootstrap an `admin` superuser with a known password — `Skip` would
    // leave us with nothing to log in as.
    let password = b"correct horse battery staple".to_vec();
    let bootstrap = BootstrapMode::Password {
        username: "admin".into(),
        password: Zeroizing::new(password.clone()),
    };

    let launcher = ServerLauncher { config, bootstrap };
    let handle = launcher.launch().await.expect("launcher boot");

    let server_addr = handle
        .first_tls_exporter_addr()
        .expect("at least one tls_exporter listener should be bound");

    // -----------------------------------------------------------------
    // Client side: real TLS + SCRAM
    // -----------------------------------------------------------------
    let username = NormalizedUsername::from_raw("admin").expect("username");

    let client_cfg = make_client_config_no_ca();
    let connector = TlsConnector::from(client_cfg);
    let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let tcp = TcpStream::connect(server_addr).await.expect("connect");
    let tls = connector.connect(server_name, tcp).await.expect("tls handshake");
    let exporter = extract_tls_exporter(&tls).expect("client exporter");

    // --- Trust-on-first-use pin: the client doesn't know the server's
    // Ed25519 pub-key yet, so we capture it during the first handshake by
    // passing a non-panicking `pin_handler` that records the value.
    let pinned: Arc<std::sync::Mutex<Option<[u8; 32]>>> = Arc::new(std::sync::Mutex::new(None));
    let pinned_for_capture = pinned.clone();

    let (mut r, mut w) = split(tls);

    // We need the server pub-key BEFORE process_auth_ok validates the
    // identity sig, because `HandshakeBuilder::pinned_hash` is required.
    // Strategy: read auth_ok bytes first, peek `server_pub_key`, then run
    // `process_auth_ok` with that as the pin.
    //
    // The simpler path: build the HandshakeBuilder with a placeholder pin
    // that the process_auth_ok callback verifies — but the API requires
    // the pin up-front. So we drive the SCRAM flow manually: send
    // auth_init, receive challenge, derive proof, send proof, receive
    // auth_ok, THEN take server_pub_key.sha256 as the pin and call
    // process_auth_ok with `|pin| { *pinned.lock() = Some(pin); }`.

    // Step 1 — auth_init. First-time client: TOFU mode (no pin yet,
    // `accept_new_host(true)`). The `process_auth_ok` callback delivers
    // the real Ed25519 pub-key hash, which we capture for assertion.
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
    let init_wire = WireAuthInit {
        user: init.user,
        client_nonce: init.client_nonce.to_vec(),
        binding_mode: init.binding_mode,
        version: init.version,
    };
    write_frame(&mut w, &rmp_serde::to_vec(&init_wire).unwrap())
        .await
        .expect("send auth_init");

    // Step 2 — challenge.
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

    // Step 3 — derive proof.
    let mut password_buf = password.clone();
    let (proof, derived, am) = hs
        .process_challenge(&challenge, &mut password_buf)
        .expect("process challenge");

    // Step 4 — send proof.
    let proof_wire = WireClientProof { client_proof: proof.to_vec() };
    write_frame(&mut w, &rmp_serde::to_vec(&proof_wire).unwrap())
        .await
        .expect("send proof");

    // Step 5 — receive auth_ok.
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

    // Now we know the server's actual pub-key. Compute the pin we'll
    // accept and feed it into the builder via `with_pinned_hash` before
    // process_auth_ok... actually `HandshakeBuilder` is consumed at build,
    // so we work around by accepting whatever the callback delivers and
    // verifying it ourselves. process_auth_ok only fires the callback when
    // the placeholder pin doesn't match (which it won't, since it's all-
    // zero). The callback receives the real pin as `[u8; 32]` — we record
    // it and treat it as the trust-decision step.
    // Server is expected to issue a resumption ticket — assert it does so
    // we don't silently regress §5.4 / SESSION_RESUMPTION wiring.
    assert!(
        !ok_wire.resumption_ticket.is_empty(),
        "server must issue a resumption ticket in auth_ok",
    );
    // Ticket expiry is computed by `issue_initial_ticket` from the same
    // wall-clock as the session expiry; both use a 24h TTL, so they are
    // ~equal. Just assert the ticket carries a non-zero TTL.
    assert!(
        ok_wire.resumption_expires_at_ns >= ok_wire.expires_at_ns
            || ok_wire.resumption_expires_at_ns > 0,
        "ticket must carry an expiry timestamp",
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

    // The callback below is only invoked if the placeholder pin doesn't
    // match the server pub-key hash — which, since it's all-zero, will
    // happen ALWAYS. We accept the pin (it's the trust decision a real
    // first-time client would make).
    let success = hs
        .process_auth_ok(&auth_ok, &derived, &am, |pin| {
            *pinned_for_capture.lock().unwrap() = Some(*pin);
        })
        .expect("process auth_ok");
    assert_eq!(success.session_id, session_id);

    // Sanity: the captured pin is sha256(server_pub_key).
    let captured = pinned.lock().unwrap().expect("TOFU callback fired");
    assert_eq!(captured, sha256(&pub32));

    // -----------------------------------------------------------------
    // Post-handshake: actually USE the database over the wire.
    //
    // Step A: against the always-present `__system__` db, create `prod`.
    // Step B: against `prod`, create repo+table, insert, read back.
    // -----------------------------------------------------------------

    let mut next_rid: u32 = 1;

    // --- Step A: create `prod` db ---
    let mk_db: shamir_db::db::query::batch::BatchRequest = serde_json::from_value(json!({
        "id": "mk-db",
        "queries": { "mk": { "create_db": "prod" } }
    })).expect("parse mk batch");
    let req_a = DbRequest::Execute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: mk_db,
    };
    let resp_a = roundtrip(&req_a, session_id, &mut next_rid, &mut w, &mut r).await;
    match resp_a {
        DbResponse::Batch { response } => {
            assert!(response.results.contains_key("mk"), "create_db ok");
        }
        other => panic!("step A expected Batch, got {:?}", other),
    }

    // --- Step B: create repo+table, set, then read ---
    let work: shamir_db::db::query::batch::BatchRequest = serde_json::from_value(json!({
        "id": "work",
        "queries": {
            "mr": { "create_repo": "main" },
            "tb": { "create_table": "items", "repo": "main" }
        }
    })).expect("parse work batch");
    let req_b = DbRequest::Execute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: "prod".into(),
        batch: work,
    };
    let resp_b = roundtrip(&req_b, session_id, &mut next_rid, &mut w, &mut r).await;
    match resp_b {
        DbResponse::Batch { response } => {
            assert!(response.results.contains_key("mr"), "create_repo ok");
            assert!(response.results.contains_key("tb"), "create_table ok");
        }
        other => panic!("step B expected Batch, got {:?}", other),
    }

    // --- Step C: write a record, read it back ---
    let rw: shamir_db::db::query::batch::BatchRequest = serde_json::from_value(json!({
        "id": "rw",
        "queries": {
            "ins": { "set": "items", "key": {"sku":"X1"}, "value": {"sku":"X1","qty":42} },
            "rd":  { "from": "items" }
        }
    })).expect("parse rw batch");
    let req_c = DbRequest::Execute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: "prod".into(),
        batch: rw,
    };
    let resp_c = roundtrip(&req_c, session_id, &mut next_rid, &mut w, &mut r).await;
    match resp_c {
        DbResponse::Batch { response } => {
            let rd = response.results.get("rd").expect("rd alias");
            assert_eq!(rd.records.len(), 1, "one record present");
            assert_eq!(rd.records[0].get("sku").and_then(|v| v.as_str()), Some("X1"));
            assert_eq!(rd.records[0].get("qty").and_then(|v| v.as_i64()), Some(42));
        }
        other => panic!("step C expected Batch, got {:?}", other),
    }

    // Clean shutdown.
    let _ = w.shutdown().await;
    let mut tmp = [0u8; 1];
    let _ = r.read(&mut tmp).await;
    handle.shutdown().await;
}
