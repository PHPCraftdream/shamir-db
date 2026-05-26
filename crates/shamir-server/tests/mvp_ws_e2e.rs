//! End-to-end test over the WebSocket transport (native WSS, binding_mode 0x01).
//!
//! Mirrors `tests/mvp_e2e.rs` but routes everything through `wss://`:
//! `tokio_tungstenite::connect_async_tls_with_config` performs the TLS
//! handshake AND the WebSocket upgrade in one step, then we drive the
//! same SCRAM flow via length-prefix-over-WS-binary framing. The TLS
//! exporter is extracted from the inner `tokio_rustls::client::TlsStream`
//! exposed by `MaybeTlsStream::Rustls`.
//!
//! What this proves:
//!   1. Server accepts a client on the WS listener (`kind=ws +
//!      profile=tls_exporter`, path `/shamir/v1`).
//!   2. The Framer abstraction in `crate::framer` correctly drives WS
//!      frames through `connection::handle_connection` — same `auth_init`
//!      / `challenge` / `client_proof` / `auth_ok` ceremony as TCP.
//!   3. Post-handshake `RequestEnvelope` / `ResponseEnvelope` round-trip
//!      works over WS just like over TCP.
//!   4. Channel binding (TLS exporter) is extracted on the client side
//!      from the underlying TLS stream BEFORE WS frames start, mirroring
//!      what the server-side `accept_loop_ws_native` does.
//!
//! WS framing reuses `shamir_transport_ws::framing::{ws_send, ws_recv_into}`
//! — exactly what `WsFramer` does on the server side.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tempfile::TempDir;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{Connector, MaybeTlsStream};
use zeroize::Zeroizing;

use shamir_connect::client::handshake::{HandshakeBuilder, ServerAuthOk, ServerChallenge};
use shamir_connect::common::envelope::{RequestEnvelope, ResponseEnvelope};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::common::username::NormalizedUsername;

use shamir_transport_tcp::tls::{extract_tls_exporter, make_client_config_no_ca};
use shamir_transport_ws::framing::{ws_recv_into, ws_send, MAX_WS_FRAME_SIZE};

use shamir_server::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ProfileKind, TlsConfig,
};
use shamir_server::db_handler::{DbRequest, DbResponse};
use shamir_server::server::{BootstrapMode, ServerLauncher};

// --------------------------------------------------------------------------
// Wire frames (mirror connection.rs's `wire` mod, transport-binding-local).
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

fn make_test_config(temp: &TempDir) -> Config {
    let data_dir = temp.path().to_path_buf();
    Config {
        data_dir: data_dir.clone(),
        logging: LoggingConfig {
            level: "warn".into(),
            slow_query_threshold_ms: 0,
        },
        kdf_defaults: fast_kdf(),
        argon2_concurrent_max: 4,
        listeners: vec![ListenerConfig {
            kind: ListenerKind::Ws,
            addr: "127.0.0.1:0".to_string(),
            profile: ProfileKind::TlsExporter,
            // Native WSS endpoint per spec TRANSPORT_WS §2.
            path: Some("/shamir/v1".to_string()),
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

// --------------------------------------------------------------------------
// Test
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mvp_full_pipeline_ws_native_tls_scram_batch_query() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().expect("tempdir");
    let config = make_test_config(&temp);

    let password = b"correct horse battery staple".to_vec();
    let bootstrap = BootstrapMode::Password {
        username: "admin".into(),
        password: Zeroizing::new(password.clone()),
    };
    let launcher = ServerLauncher { config, bootstrap };
    let handle = launcher.launch().await.expect("launcher boot");
    let server_addr = handle
        .first_tls_exporter_addr()
        .expect("WS listener should be bound");

    // ----- Client: TLS+WS upgrade in one go -----
    let client_cfg = make_client_config_no_ca();
    let connector = Connector::Rustls(client_cfg);
    let url = format!("wss://localhost:{}/shamir/v1", server_addr.port());

    // tokio_tungstenite resolves "localhost" via DNS; we want to land on
    // the actual bound IP (which is 127.0.0.1 from `addr: 127.0.0.1:0`).
    // Pre-build a TcpStream and pass it via `client_async_tls_with_config`.
    let tcp = tokio::net::TcpStream::connect(server_addr)
        .await
        .expect("tcp");
    let req = url.into_client_request().expect("uri");

    let (ws, _resp) = tokio_tungstenite::client_async_tls_with_config(
        req,
        tcp,
        None, // request config
        Some(connector),
    )
    .await
    .expect("ws+tls upgrade");

    // Extract the TLS exporter from the underlying client TLS stream —
    // identical channel-binding logic to the server-side
    // `accept_loop_ws_native`.
    let exporter = match ws.get_ref() {
        MaybeTlsStream::Rustls(tls) => {
            extract_tls_exporter(tls).expect("exporter must be extractable on TLS 1.3")
        }
        other => panic!("expected MaybeTlsStream::Rustls, got {:?}", other),
    };

    let mut ws = ws;

    // ----- SCRAM via WS frames -----
    let username = NormalizedUsername::from_raw("admin").expect("username");
    let hs = HandshakeBuilder::new(
        username.clone(),
        TransportKind::WebSocket,
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
    ws_send(&mut ws, &rmp_serde::to_vec(&init_wire).unwrap())
        .await
        .expect("send auth_init");

    // challenge
    let mut buf = Vec::new();
    ws_recv_into(&mut ws, MAX_WS_FRAME_SIZE, &mut buf)
        .await
        .expect("read challenge");
    let ch_wire: WireChallenge = rmp_serde::from_slice(&buf).expect("decode challenge");
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
    let mut password_buf = password.clone();
    let (proof, derived, am) = hs
        .process_challenge(&challenge, &mut password_buf)
        .expect("process challenge");
    let proof_wire = WireClientProof {
        client_proof: proof.to_vec(),
    };
    ws_send(&mut ws, &rmp_serde::to_vec(&proof_wire).unwrap())
        .await
        .expect("send proof");

    // auth_ok
    ws_recv_into(&mut ws, MAX_WS_FRAME_SIZE, &mut buf)
        .await
        .expect("read auth_ok");
    let ok_wire: WireAuthOk = rmp_serde::from_slice(&buf).expect("decode auth_ok");
    let mut sig32 = [0u8; 32];
    sig32.copy_from_slice(&ok_wire.server_signature);
    let mut pub32 = [0u8; 32];
    pub32.copy_from_slice(&ok_wire.server_pub_key);
    let mut id_sig = [0u8; 64];
    id_sig.copy_from_slice(&ok_wire.identity_sig);
    let mut session_id = [0u8; 32];
    session_id.copy_from_slice(&ok_wire.session_id);

    let pinned = Arc::new(std::sync::Mutex::new(None::<[u8; 32]>));
    let pinned_for_capture = pinned.clone();
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
    assert!(pinned.lock().unwrap().is_some(), "TOFU pin captured");

    // ----- Batch A: create the table (admin op) -----
    // create_table and insert can NOT live in the same batch because the
    // planner runs independent ops in a single parallel stage and the
    // insert can race ahead of the create — same constraint as mvp_e2e.
    let mk: shamir_db::query::batch::BatchRequest = serde_json::from_value(json!({
        "id": "ws-mk",
        "queries": { "tb": { "create_table": "ws_items", "repo": "main" } }
    }))
    .expect("parse batch");
    let req_a = DbRequest::Execute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: mk,
    };
    let env_a = RequestEnvelope::new(
        session_id,
        Some(41),
        rmp_serde::to_vec_named(&req_a).unwrap(),
    );
    ws_send(&mut ws, &env_a.to_msgpack().unwrap())
        .await
        .expect("send mk");
    ws_recv_into(&mut ws, MAX_WS_FRAME_SIZE, &mut buf)
        .await
        .expect("read mk");
    let resp = ResponseEnvelope::from_msgpack(&buf).expect("env mk");
    let db_resp: DbResponse = rmp_serde::from_slice(&resp.res).expect("decode mk");
    match &db_resp {
        DbResponse::Batch { .. } => {}
        other => panic!("create_table failed: {:?}", other),
    }

    // ----- Batch B: insert + read in one go (those two are safe because
    // the read sees the insert from the same stage when we have only one
    // table involved — but to be deterministic we issue them in two
    // separate batches as well). -----
    let ins: shamir_db::query::batch::BatchRequest = serde_json::from_value(json!({
        "id": "ws-ins",
        "queries": { "ins": { "set": "ws_items", "key": {"id":"a"}, "value": {"id":"a","n":7} } }
    }))
    .expect("parse batch");
    let req_b = DbRequest::Execute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: ins,
    };
    let env_b = RequestEnvelope::new(
        session_id,
        Some(42),
        rmp_serde::to_vec_named(&req_b).unwrap(),
    );
    ws_send(&mut ws, &env_b.to_msgpack().unwrap())
        .await
        .expect("send ins");
    ws_recv_into(&mut ws, MAX_WS_FRAME_SIZE, &mut buf)
        .await
        .expect("read ins");
    let resp = ResponseEnvelope::from_msgpack(&buf).expect("env ins");
    let db_resp: DbResponse = rmp_serde::from_slice(&resp.res).expect("decode ins");
    match &db_resp {
        DbResponse::Batch { .. } => {}
        other => panic!("insert failed: {:?}", other),
    }

    let rd: shamir_db::query::batch::BatchRequest = serde_json::from_value(json!({
        "id": "ws-rd",
        "queries": { "rd": { "from": "ws_items" } }
    }))
    .expect("parse batch");
    let req_c = DbRequest::Execute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: rd,
    };
    let env_c = RequestEnvelope::new(
        session_id,
        Some(43),
        rmp_serde::to_vec_named(&req_c).unwrap(),
    );
    ws_send(&mut ws, &env_c.to_msgpack().unwrap())
        .await
        .expect("send rd");
    ws_recv_into(&mut ws, MAX_WS_FRAME_SIZE, &mut buf)
        .await
        .expect("read rd");
    let resp = ResponseEnvelope::from_msgpack(&buf).expect("env rd");
    assert_eq!(resp.request_id, Some(43), "request_id echoed");

    let db_resp: DbResponse = rmp_serde::from_slice(&resp.res).expect("decode rd");
    match db_resp {
        DbResponse::Batch { response } => {
            let rd = response.results.get("rd").expect("rd alias");
            assert_eq!(rd.records.len(), 1, "exactly one record");
            assert_eq!(rd.records[0].get("id").and_then(|v| v.as_str()), Some("a"));
            assert_eq!(rd.records[0].get("n").and_then(|v| v.as_i64()), Some(7));
        }
        other => panic!("expected Batch, got {:?}", other),
    }

    // Graceful close.
    let _ = ws.close(None).await;
    // Drain server's close frame (best effort; timeout to avoid hang).
    let _ = tokio::time::timeout(
        Duration::from_millis(200),
        ws_recv_into(&mut ws, MAX_WS_FRAME_SIZE, &mut buf),
    )
    .await;
    handle.shutdown().await;
}
