//! Shared online-backup fixture: boots the real server against a caller
//! owned data directory, connects a typed client, drives the fixture
//! DDL/data through the typed builder surface, and shuts the server down
//! with a validated clean report.
//!
//! Same known-good boot/connect/shutdown pattern as
//! `rust_migration_tests.rs` (release 48, step 3a); the DDL/index writer
//! shapes mirror `examples/reference_fixture.rs`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use zeroize::Zeroizing;

use shamir_client::builder::batch::Batch;
use shamir_client::builder::ddl::{self, RepoEngine};
use shamir_client::{Client, ClientError, ConnectOptions};
use shamir_query_types::batch::{BatchRequest, BatchResponse};

use shamir_server::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ProfileKind, SecurityConfig,
    TlsConfig,
};
use shamir_server::server::{BootstrapMode, ServerHandle, ServerLauncher};

pub(super) const DB: &str = "backup_load";
pub(super) const REPO: &str = "main";
pub(super) const TABLE: &str = "rows";

/// Per-request deadline, shared by every awaited client operation and by
/// `paced_execute`'s retry budget (10× the 12 s baseline).
pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

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
        // 64: headroom alignment, NOT an EOF fix — SCRAM never touches the
        // argon2 semaphore (the client KDF runs only after the challenge).
        argon2_concurrent_max: 64,
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
        // Loopback fixture only: the auth_init limiter (shamir-sync
        // handshake.rs, BEFORE the first frame) defaults to 10/s per subnet,
        // burst 2 (startup divides the default by 4). Five sequential
        // same-subnet clients can finish inside 1.4 s and exhaust it, so
        // raise it to 64 (effective 16/s during warmup — above every gate
        // connection here) while keeping the throttle finite.
        security: SecurityConfig {
            auth_init_rate_per_second: 64,
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

/// Connect a typed client as the bootstrap admin. Every await inside is
/// bounded by [`REQUEST_TIMEOUT`], which is also installed as the client's
/// per-request deadline (along with explicit connect/handshake timeouts —
/// `None` would fall back to the client's own tight defaults).
///
/// `label` is a scenario tag surfaced in both diagnostics (deadline expiry
/// and the typed [`ClientError`]); there are NO retries here.
pub async fn connect(
    addr: std::net::SocketAddr,
    password: &Zeroizing<Vec<u8>>,
    label: &str,
) -> Client {
    let connect = Box::pin(Client::connect(ConnectOptions {
        addr,
        server_name: "localhost".into(),
        username: ADMIN_USER.into(),
        password: Zeroizing::new(password.to_vec()),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: Some(REQUEST_TIMEOUT),
        handshake_timeout: Some(REQUEST_TIMEOUT),
        request_timeout: Some(REQUEST_TIMEOUT),
    }));
    match tokio::time::timeout(REQUEST_TIMEOUT, connect).await {
        Ok(Ok(client)) => client,
        Ok(Err(e)) => panic!("[{label}] client connect failed: {e}"),
        Err(_elapsed) => {
            panic!("[{label}] client connect exceeded the {REQUEST_TIMEOUT:?} deadline")
        }
    }
}

/// Execute a batch with pacing and bounded `rate_limited` retries.
///
/// Wire contract (dispatch.rs pre-handler rejection):
/// * sleeps `pace` before EVERY attempt (including the first);
/// * retries ONLY `ClientError::Db { code: "rate_limited", .. }` — the
///   server's pre-handler limiter rejection; transport, handshake,
///   protocol and timeout errors are returned immediately, never retried;
/// * at most 3 attempts, bounded by ONE genuine [`REQUEST_TIMEOUT`]
///   deadline (`timeout_at` with `tokio::time::Instant`) covering BOTH the
///   pacing sleeps and each complete wire attempt — a deadline hit never
///   retries and returns a typed `Io(TimedOut)` error instead. Dropping
///   the in-flight future at expiry also traces the client's pending
///   response cancellation; because the server may still apply a batch
///   that was already written to the wire, a deadline expiry leaves the
///   commit AMBIGUOUS (the client's pending-response wait is cancelled by
///   the mid-write drop and the pending registration is released by RAII
///   in the client api) — callers must not infer outcome from `TimedOut`;
/// * the request is sent as a cloned owned [`BatchRequest`], so the
///   caller's builder value stays untouched across retries.
///
/// Returns the std `Instant` stamped AFTER pacing/backoff, immediately before
/// the actual attempt (the successful one on `Ok`) — so overlap proofs
/// never include the pacing sleep.
pub(super) async fn paced_execute(
    client: &Client,
    db: &str,
    batch: BatchRequest,
    pace: Duration,
) -> (Instant, Result<BatchResponse, ClientError>) {
    const MAX_ATTEMPTS: u32 = 3;
    let deadline = tokio::time::Instant::now() + REQUEST_TIMEOUT;

    let timed_out = |where_: &str| {
        ClientError::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("paced_execute exceeded the {REQUEST_TIMEOUT:?} deadline {where_}"),
        ))
    };

    let mut stamp = Instant::now();
    for attempt in 0..MAX_ATTEMPTS {
        // Pacing sleep, bounded by the same single deadline.
        let wake = tokio::time::Instant::now() + pace;
        tokio::time::sleep_until(wake.min(deadline)).await;
        // `timeout_at` may poll the inner future even when the deadline
        // has already expired; never touch the wire on an expired budget.
        if tokio::time::Instant::now() >= deadline {
            return (stamp, Err(timed_out("before a wire attempt")));
        }
        stamp = Instant::now();
        // Complete wire attempt under the deadline; dropping the future
        // at expiry cancels the client's pending-response wait.
        match tokio::time::timeout_at(deadline, client.execute(db, batch.clone())).await {
            Ok(Ok(response)) => return (stamp, Ok(response)),
            Ok(Err(e))
                if matches!(&e, ClientError::Db { code, .. } if code == "rate_limited")
                    && attempt + 1 < MAX_ATTEMPTS => {}
            Ok(Err(e)) => return (stamp, Err(e)),
            Err(_elapsed) => {
                return (stamp, Err(timed_out(&format!("at attempt {attempt}"))));
            }
        }
    }
    unreachable!("retry loop returns on every path")
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
