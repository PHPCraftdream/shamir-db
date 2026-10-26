//! Shared fixtures for the backup/restore e2e tests: wire structs, server
//! config/launch, TLS+SCRAM login, request round-trip and query builders.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;
use tokio::io::split;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use zeroize::Zeroizing;

use shamir_connect::client::handshake::{HandshakeBuilder, ServerAuthOk, ServerChallenge};
use shamir_connect::common::envelope::{RequestEnvelope, ResponseEnvelope};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::common::username::NormalizedUsername;

use shamir_transport_tcp::framing::{read_frame, write_frame, MAX_FRAME_SIZE_DEFAULT};
use shamir_transport_tcp::tls::{
    extract_tls_exporter, make_client_config_app_authenticated, AppLayerServerAuth,
};

use shamir_server::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ObservabilityConfig,
    ProfileKind, TlsConfig,
};
use shamir_server::db_handler::{DbRequest, DbResponse};
use shamir_server::server::{BootstrapMode, ServerHandle, ServerLauncher};

#[derive(Serialize, Deserialize)]
pub(crate) struct WireAuthInit {
    pub(crate) user: String,
    #[serde(with = "serde_bytes")]
    pub(crate) client_nonce: Vec<u8>,
    pub(crate) binding_mode: u8,
    pub(crate) version: u8,
}
#[derive(Serialize, Deserialize)]
pub(crate) struct WireChallenge {
    #[serde(with = "serde_bytes")]
    pub(crate) salt: Vec<u8>,
    pub(crate) memory_kb: u32,
    pub(crate) time: u32,
    pub(crate) parallelism: u32,
    pub(crate) argon2_version: u8,
    #[serde(with = "serde_bytes")]
    pub(crate) server_nonce: Vec<u8>,
}
#[derive(Serialize, Deserialize)]
pub(crate) struct WireClientProof {
    #[serde(with = "serde_bytes")]
    pub(crate) client_proof: Vec<u8>,
}
#[derive(Serialize, Deserialize)]
pub(crate) struct WireAuthOk {
    #[serde(with = "serde_bytes")]
    pub(crate) server_signature: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub(crate) server_pub_key: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub(crate) identity_sig: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub(crate) session_id: Vec<u8>,
    pub(crate) expires_at_ns: u64,
    #[serde(default, with = "serde_bytes")]
    pub(crate) resumption_ticket: Vec<u8>,
    #[serde(default)]
    pub(crate) resumption_expires_at_ns: u64,
    #[serde(default)]
    pub(crate) server_query_version: u8,
}

pub(crate) fn fast_kdf() -> KdfConfig {
    KdfConfig {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

pub(crate) fn make_config(data_dir: PathBuf, port: u16) -> Config {
    Config {
        durability: Default::default(),
        transactions: Default::default(),
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

pub(crate) async fn launch(data_dir: PathBuf, port: u16, password: &[u8]) -> ServerHandle {
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

pub(crate) async fn login(
    addr: std::net::SocketAddr,
    password: &[u8],
) -> (
    tokio::io::ReadHalf<tokio_rustls::client::TlsStream<TcpStream>>,
    tokio::io::WriteHalf<tokio_rustls::client::TlsStream<TcpStream>>,
    [u8; 32],
) {
    let username = NormalizedUsername::from_raw("admin").unwrap();
    let connector = TlsConnector::from(
        make_client_config_app_authenticated(AppLayerServerAuth::exporter_bound()).unwrap(),
    );
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

pub(crate) async fn roundtrip<R, W>(
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

pub(crate) fn create_table_req(name: &str) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("tbl");
    b.create_table("tb", shamir_query_builder::ddl::create_table(name));
    DbRequest::Execute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: b.build(),
    }
}
pub(crate) fn write_req(table: &str, sku: &str, qty: i64) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("wr");
    b.try_upsert(
        "ins",
        shamir_query_builder::write::upsert(table)
            .key(mpack!({"sku": @(QueryValue::from(sku))}))
            .value(shamir_query_builder::doc! { "sku" => sku, "qty" => qty }),
    )
    .unwrap();
    DbRequest::Execute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: b.build(),
    }
}
pub(crate) fn read_req(table: &str) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("rd");
    b.query("rd", shamir_query_builder::Query::from(table));
    DbRequest::Execute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: b.build(),
    }
}
