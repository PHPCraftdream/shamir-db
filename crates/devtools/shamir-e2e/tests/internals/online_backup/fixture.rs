//! Shared online-backup fixture: boots the real server against a caller
//! owned data directory, connects a typed client, drives the fixture
//! DDL/data through the typed builder surface, and shuts the server down
//! with a validated clean report.
//!
//! Same known-good boot/connect/shutdown pattern as
//! `rust_migration_tests.rs` (release 48, step 3a); the DDL/index writer
//! shapes mirror `examples/reference_fixture.rs`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use zeroize::Zeroizing;

use shamir_client::builder::batch::Batch;
use shamir_client::builder::ddl::{self, RepoEngine};
use shamir_client::{Client, ConnectOptions};

use shamir_server::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ProfileKind, TlsConfig,
};
use shamir_server::server::{BootstrapMode, ServerHandle, ServerLauncher};

pub(super) const DB: &str = "backup_load";
pub(super) const REPO: &str = "main";
pub(super) const TABLE: &str = "rows";

pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

const ADMIN_USER: &str = "admin";
const ADMIN_PASSWORD: &[u8] = b"correct horse battery staple";

/// Everything a scenario needs after `boot`: the running server handle and
/// the bootstrap credentials (the same credentials reconnect to an EXISTING
/// directory, since that directory already has its superuser).
pub struct Booted {
    pub handle: ServerHandle,
    pub password: Zeroizing<Vec<u8>>,
}

impl Booted {
    pub fn addr(&self) -> std::net::SocketAddr {
        self.handle
            .first_tls_exporter_addr()
            .expect("bound TLS exporter listener")
    }
}

fn backup_config(data_dir: &Path) -> Config {
    let data_dir: PathBuf = data_dir.to_path_buf();
    Config {
        tokens: Default::default(),
        data_dir: data_dir.clone(),
        logging: LoggingConfig {
            level: "warn".into(),
            slow_query_threshold_ms: 0,
            file: None,
            flush_interval_ms: 2000,
        },
        durability: Default::default(),
        transactions: Default::default(),
        kdf_defaults: KdfConfig {
            memory_kb: 19_456,
            time: 2,
            parallelism: 1,
            argon2_version: 0x13,
        },
        argon2_concurrent_max: 4,
        listeners: vec![ListenerConfig {
            kind: ListenerKind::Tcp,
            addr: "127.0.0.1:0".into(),
            profile: Some(ProfileKind::TlsExporter),
            path: None,
            kdf_override: None,
            browser_origin_allowlist: vec![],
            http: Default::default(),
        }],
        tls: TlsConfig {
            cert_path: data_dir.join("cert.pem"),
            key_path: data_dir.join("cert.key"),
        },
        security: Default::default(),
        audit: Default::default(),
        observability: shamir_server::config::ObservabilityConfig {
            addr: String::new(),
            allow_public_metrics: false,
        },
        replication: None,
    }
}

/// Boot the real server on `data_dir`. A FRESH directory bootstraps the
/// superuser with the fixture password (`BootstrapMode::Password`); an
/// EXISTING directory skips bootstrap entirely (`BootstrapMode::Skip`) —
/// the superuser is already there and the same fixture password reconnects.
pub async fn boot(data_dir: &Path, fresh: bool) -> Booted {
    // aws-lc must be installed before any TLS handshake (existing fixture
    // pattern of `rust_migration_tests` / `smoke_local`).
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let password = Zeroizing::new(ADMIN_PASSWORD.to_vec());
    let bootstrap = if fresh {
        BootstrapMode::Password {
            username: ADMIN_USER.into(),
            password: Zeroizing::new(ADMIN_PASSWORD.to_vec()),
        }
    } else {
        BootstrapMode::Skip
    };

    let launcher = ServerLauncher {
        node_mode: Default::default(),
        config: backup_config(data_dir),
        bootstrap,
    };
    // Launch is Box::pinned so deep server-side frames live on the heap.
    let handle: ServerHandle = tokio::time::timeout(REQUEST_TIMEOUT, Box::pin(launcher.launch()))
        .await
        .expect("server launch deadline")
        .expect("server launch");
    Booted { handle, password }
}

/// Connect a typed client as the bootstrap admin; every await inside is
/// bounded by `REQUEST_TIMEOUT` (also set as the request deadline).
pub async fn connect(addr: std::net::SocketAddr, password: &Zeroizing<Vec<u8>>) -> Client {
    tokio::time::timeout(
        REQUEST_TIMEOUT,
        Box::pin(Client::connect(ConnectOptions {
            addr,
            server_name: "localhost".into(),
            username: ADMIN_USER.into(),
            password: Zeroizing::new(password.to_vec()),
            accept_new_host: true,
            trusted_pin: None,
            connect_timeout: None,
            handshake_timeout: None,
            request_timeout: Some(REQUEST_TIMEOUT),
        })),
    )
    .await
    .expect("client connect deadline")
    .expect("client connect")
}

/// Close the client and shut the server down; the report must be clean
/// (the persistent Fjall repo included).
pub async fn shutdown(client: Client, booted: Booted) {
    tokio::time::timeout(REQUEST_TIMEOUT, client.close())
        .await
        .expect("client close deadline");
    let report = tokio::time::timeout(REQUEST_TIMEOUT, Box::pin(booted.handle.shutdown()))
        .await
        .expect("server shutdown deadline");
    assert!(
        report.is_clean(),
        "clean shutdown, including the persistent repo: {:?}",
        report.repository_failures()
    );
}

/// Typed fixture setup on a running server: the `{DB}` database, a
/// PERSISTENT Fjall repo (explicit durable engine — real persisted home for
/// the backup), the `{TABLE}` rows table and its hash index.
///
/// Same batch shapes as `rust_migration_tests.rs`; the hash index writer is
/// the `examples/reference_fixture.rs` shape
/// (`create_index(..).repo(REPO).hash(...)` with the typed spec error
/// mapped out).
pub async fn setup(client: &Client) {
    let mut mk = Batch::new();
    mk.id("mk-db");
    mk.create_db(DB, ddl::create_db(DB));
    tokio::time::timeout(REQUEST_TIMEOUT, client.execute("default", mk.build()))
        .await
        .expect("create db deadline")
        .expect("create db");

    let mut repo = Batch::new();
    repo.id("mk-repo");
    repo.create_repo(REPO, ddl::create_repo(REPO).engine(RepoEngine::Fjall));
    tokio::time::timeout(REQUEST_TIMEOUT, client.execute(DB, repo.build()))
        .await
        .expect("create repo deadline")
        .expect("create repo");

    let mut table = Batch::new();
    table.id("mk-table");
    table.create_table(TABLE, ddl::create_table(TABLE).repo(REPO));
    tokio::time::timeout(REQUEST_TIMEOUT, client.execute(DB, table.build()))
        .await
        .expect("create table deadline")
        .expect("create table");

    let mut index = Batch::new();
    index.id("mk-index");
    index.create_index(
        "idx-hash",
        ddl::create_index("rows_hash_idx", TABLE)
            .repo(REPO)
            .hash(vec![vec!["writer".into()]])
            .map_err(|e| format!("hash index spec invalid: {e}"))
            .expect("hash index spec validates"),
    );
    tokio::time::timeout(REQUEST_TIMEOUT, client.execute(DB, index.build()))
        .await
        .expect("create index deadline")
        .expect("create index");
}
