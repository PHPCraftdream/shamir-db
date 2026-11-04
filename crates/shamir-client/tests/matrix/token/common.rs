//! Token REST fixture: one live server with a TLS and a plaintext loopback
//! HTTP listener, a seeded owner and one read-write plus one read-only token.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::token::{TokenChannels, TokenMode};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_types::access::ResourcePath;
use tempfile::TempDir;
use zeroize::Zeroizing;

use shamir_client::builder::batch::Batch;
use shamir_client::builder::ddl;
use shamir_client::builder::doc;
use shamir_client::builder::write::insert;
use shamir_client::builder::Query;
use shamir_client::{
    ApiToken, BatchRequest, Client, ConnectOptions, TokenEndpoint, TokenHttpClient,
    TokenHttpOptions, TokenTrust,
};

use shamir_server::bootstrap::{ensure_superuser, BootstrapPolicy};
use shamir_server::config::{
    Config, HttpListenerSettings, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig,
    ObservabilityConfig, ProfileKind, SecurityConfig, TlsConfig,
};
use shamir_server::server::{BootstrapMode, ServerHandle, ServerLauncher};
use shamir_server::tokens::NewToken;
use shamir_server::user_directory::FjallUserDirectory;

const ADMIN_PASSWORD: &[u8] = b"token-matrix-admin-password";
const SERVER_NAME: &str = "localhost";

pub struct TokenFixture {
    pub handle: ServerHandle,
    pub tls: SocketAddr,
    pub plain: SocketAddr,
    pub read_write: String,
    pub read_only: String,
    cert_pem: Vec<u8>,
    owner: [u8; 16],
    owner_roles: Vec<String>,
    _temp: TempDir,
}

impl TokenFixture {
    pub async fn shutdown(self) {
        self.handle.shutdown().await;
    }

    pub fn tls_endpoint(&self) -> TokenEndpoint {
        let trust = TokenTrust::pem(&self.cert_pem).expect("trust generated cert");
        TokenEndpoint::https(self.tls, SERVER_NAME, trust)
    }

    pub fn untrusted_endpoint(&self) -> TokenEndpoint {
        TokenEndpoint::https(self.tls, SERVER_NAME, TokenTrust::webpki_roots())
    }

    pub fn plain_endpoint(&self) -> TokenEndpoint {
        TokenEndpoint::plain_loopback(self.plain).expect("loopback endpoint")
    }

    pub async fn connect_tls(&self, token: &str) -> TokenHttpClient {
        connect(self.tls_endpoint(), token).await
    }

    pub async fn connect_plain(&self, token: &str) -> TokenHttpClient {
        connect(self.plain_endpoint(), token).await
    }

    /// Well-formed token with a known id and an all-zero secret the server never issued.
    pub fn unknown_secret_token(&self) -> String {
        let id = self.read_write.split('.').nth(1).expect("token id segment");
        format!("shk1.{id}.{}", "A".repeat(43))
    }

    async fn issue(&self, label: &str, mode: TokenMode) -> String {
        let created = self
            .handle
            .token_service
            .create(NewToken {
                owner_user_id: self.owner,
                owner_roles: self.owner_roles.clone(),
                label: label.into(),
                ttl_secs: Some(3600),
                mode,
                channels: TokenChannels::Rest,
                databases: None,
            })
            .await
            .expect("create token");
        created.token.to_string()
    }
}

pub async fn connect(endpoint: TokenEndpoint, token: &str) -> TokenHttpClient {
    let token = ApiToken::new(token).expect("well-formed token");
    TokenHttpClient::connect(TokenHttpOptions::new(endpoint, token))
        .await
        .expect("token client connect")
}

fn kdf() -> KdfConfig {
    KdfConfig {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

fn http_listener(tls: bool) -> ListenerConfig {
    ListenerConfig {
        kind: ListenerKind::Http,
        addr: "127.0.0.1:0".into(),
        profile: None,
        path: None,
        kdf_override: None,
        browser_origin_allowlist: vec![],
        http: HttpListenerSettings {
            tls: Some(tls),
            ..Default::default()
        },
    }
}

/// Native SCRAM listener: a token may not run DDL, so the fixture creates its
/// table through an admin session on this listener.
fn native_listener() -> ListenerConfig {
    ListenerConfig {
        kind: ListenerKind::Tcp,
        addr: "127.0.0.1:0".into(),
        profile: Some(ProfileKind::TlsExporter),
        path: None,
        kdf_override: None,
        browser_origin_allowlist: vec![],
        http: Default::default(),
    }
}

fn make_config(data_dir: &Path) -> Config {
    Config {
        tokens: Default::default(),
        data_dir: data_dir.to_path_buf(),
        logging: LoggingConfig {
            level: "warn".into(),
            slow_query_threshold_ms: 0,
            file: None,
            flush_interval_ms: 2000,
        },
        durability: Default::default(),
        transactions: Default::default(),
        kdf_defaults: kdf(),
        argon2_concurrent_max: 4,
        listeners: vec![http_listener(true), http_listener(false), native_listener()],
        tls: TlsConfig {
            cert_path: data_dir.join("cert.pem"),
            key_path: data_dir.join("key.pem"),
        },
        security: SecurityConfig {
            auth_init_rate_per_second: 100_000,
            ..Default::default()
        },
        audit: Default::default(),
        observability: ObservabilityConfig {
            addr: String::new(),
            allow_public_metrics: false,
        },
        replication: None,
    }
}

async fn seed_owner(data_dir: &Path) -> ([u8; 16], Vec<String>) {
    let directory = FjallUserDirectory::open(data_dir.join("users")).expect("directory");
    let kdf = kdf();
    ensure_superuser(
        &directory,
        data_dir,
        "admin",
        BootstrapPolicy::Password(ADMIN_PASSWORD),
        &KdfParams {
            memory_kb: kdf.memory_kb,
            time: kdf.time,
            parallelism: kdf.parallelism,
            argon2_version: kdf.argon2_version,
        },
    )
    .expect("seed admin");
    let owner = UserDirectory::user_id(&directory, "admin").expect("admin owner id");
    let owner_roles = directory
        .state_by_user_id(&owner)
        .expect("seeded owner state")
        .roles;
    drop(directory);

    let db = shamir_db::ShamirDb::init(shamir_db::SystemStoreConfig::Fjall(
        data_dir.join("shamir_db_meta.redb"),
    ))
    .await
    .expect("initialize owner seed database");
    db.create_db("default")
        .await
        .expect("create default database");
    db.add_repo(
        "default",
        RepoConfig::new(
            "main",
            BoxRepoFactory::fjall(data_dir.join("shamir_db_default_main.redb")),
        ),
    )
    .await
    .expect("create default main repository");

    for (path, mode) in [
        (
            ResourcePath::Database {
                db: "default".into(),
            },
            0o757,
        ),
        (
            ResourcePath::Store {
                db: "default".into(),
                store: "main".into(),
            },
            0o711,
        ),
    ] {
        let mut meta = db
            .resource_meta(&path)
            .await
            .expect("read resource metadata");
        meta.mode = mode;
        db.set_resource_meta(&path, &meta)
            .await
            .expect("set resource mode");
    }
    db.flush_all().await.expect("flush owner seed database");
    drop(db);
    (owner, owner_roles)
}

pub async fn boot() -> TokenFixture {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().expect("tempdir");
    let data_dir: PathBuf = temp.path().to_path_buf();
    let (owner, owner_roles) = seed_owner(&data_dir).await;
    let handle = ServerLauncher {
        node_mode: Default::default(),
        config: make_config(&data_dir),
        bootstrap: BootstrapMode::Password {
            username: "admin".into(),
            password: Zeroizing::new(ADMIN_PASSWORD.to_vec()),
        },
    }
    .launch()
    .await
    .expect("launcher boot");
    let tls = handle.bound_addrs[0].expect("TLS listener bound");
    let plain = handle.bound_addrs[1].expect("plain listener bound");
    let native = handle.bound_addrs[2].expect("native listener bound");
    create_items_table(native).await;
    let cert_pem = std::fs::read(data_dir.join("cert.pem")).expect("generated cert");

    let mut fixture = TokenFixture {
        handle,
        tls,
        plain,
        read_write: String::new(),
        read_only: String::new(),
        cert_pem,
        owner,
        owner_roles,
        _temp: temp,
    };
    fixture.read_write = fixture.issue("matrix-rw", TokenMode::ReadWrite).await;
    fixture.read_only = fixture.issue("matrix-ro", TokenMode::ReadOnly).await;
    fixture
}

async fn create_items_table(native: SocketAddr) {
    let admin = Client::connect(ConnectOptions {
        addr: native,
        server_name: SERVER_NAME.into(),
        username: "admin".into(),
        password: Zeroizing::new(ADMIN_PASSWORD.to_vec()),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: Some(Duration::from_secs(10)),
        handshake_timeout: Some(Duration::from_secs(60)),
        request_timeout: Some(Duration::from_secs(30)),
    })
    .await
    .expect("admin connect");
    admin
        .execute("default", create_table_batch())
        .await
        .expect("create table");
    admin.close().await;
}

fn create_table_batch() -> BatchRequest {
    let mut batch = Batch::new();
    batch.id("mk-items");
    batch.create_table("mk", ddl::create_table("items"));
    batch.build()
}

pub fn seed_batch() -> BatchRequest {
    let mut batch = Batch::new();
    batch.id("seed-items");
    batch.insert(
        "ins",
        insert("items").rows([doc! { "marker" => "token-rest" }]),
    );
    batch.build()
}

pub fn read_batch() -> BatchRequest {
    let mut batch = Batch::new();
    batch.id("rd-items");
    batch.query("rd", Query::from("items"));
    batch.build()
}

pub fn write_batch() -> BatchRequest {
    let mut batch = Batch::new();
    batch.id("wr-items");
    batch.insert(
        "ins",
        insert("items").rows([doc! { "marker" => "forbidden" }]),
    );
    batch.build()
}
