//! Regression for the P2-U01 finding: a `WireReplSource` long poll for one
//! pull target must not serialize the other pull targets of the same
//! subscription.
//!
//! The earlier version of this test drove a hand-written fake `ReplSource`,
//! which never exercised the production `WireReplSource` path and passed
//! vacuously. This test boots a REAL leader (`ServerLauncher`), connects a
//! real `shamir_client::Client` through TLS+SCRAM, wraps it in the real
//! [`WireReplSource`], and drives the actual `Client::repl` request/response
//! path: the first pull long-polls (`wait_ms`) on an empty repo while the
//! second pull on a different repo must complete without waiting behind it.
//!
//! Against the old implementation — where `WireReplSource::pull` held a
//! source-level mutex across the whole upstream call — the second pull
//! blocked until the first long poll timed out and the test's bounded wait
//! on the second target failed.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use zeroize::Zeroizing;

use crate::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ProfileKind, TlsConfig,
};
use crate::server::{BootstrapMode, ServerLauncher};
use crate::sync::replication::transport::source::ReplSource;
use crate::sync::replication::wire_source::WireReplSource;
use shamir_client::{Client, ConnectOptions};
use shamir_query_types::wire::repl::ReplResponse;

fn leader_config(temp: &tempfile::TempDir) -> Config {
    let data_dir: PathBuf = temp.path().to_path_buf();
    Config {
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
        observability: crate::config::ObservabilityConfig {
            addr: String::new(),
            allow_public_metrics: false,
        },
        replication: None,
    }
}

async fn connect_superuser(addr: std::net::SocketAddr, password: &[u8]) -> Client {
    Client::connect(ConnectOptions {
        addr,
        server_name: "localhost".into(),
        username: "admin".into(),
        password: Zeroizing::new(password.to_vec()),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: Some(Duration::from_secs(10)),
        handshake_timeout: None,
        // Unbounded: the long-polled target is intentionally slow.
        request_timeout: None,
    })
    .await
    .expect("superuser connect")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rv_p2_u01_long_poll_does_not_serialize_other_pull_targets() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let password = b"correct horse battery staple".to_vec();
    let launcher = ServerLauncher {
        config: leader_config(&temp),
        bootstrap: BootstrapMode::Password {
            username: "admin".into(),
            password: Zeroizing::new(password.clone()),
        },
    };
    let handle = launcher.launch().await.expect("leader boot");
    let addr = handle.first_tls_exporter_addr().expect("tls listener");

    let client = connect_superuser(addr, &password).await;

    // Create the database and two empty pull targets (repos) BEFORE the
    // client is moved into the production `WireReplSource`.
    let mut mk = shamir_query_builder::batch::Batch::new();
    mk.create_db("mk", shamir_query_builder::ddl::create_db("repldb"));
    let response = client
        .execute("default", mk.build())
        .await
        .expect("create db");
    assert!(
        response.results.contains_key("mk"),
        "create_db must succeed"
    );
    for repo in ["first", "second"] {
        let mut batch = shamir_query_builder::batch::Batch::new();
        batch.create_repo("mr", shamir_query_builder::ddl::create_repo(repo));
        let response = client
            .execute("repldb", batch.build())
            .await
            .expect("create repo");
        assert!(
            response.results.contains_key("mr"),
            "create_repo {repo} must succeed"
        );
    }

    // ONE production WireReplSource shared by both pull targets — the same
    // topology the old code serialized with its source-level mutex.
    let source = Arc::new(WireReplSource::new(client));

    // Two long polls on different empty repos through the SAME source. The
    // old source-level mutex let whichever pull entered first hold the lock
    // for its whole wait, so the pair took about 2 * WAIT whatever the start
    // order; concurrent pulls take about WAIT. Timing the pair needs no head
    // start or sleep, so the check does not depend on task scheduling.
    const WAIT_MS: u32 = 4_000;
    let started = tokio::time::Instant::now();
    let first_source = source.clone();
    let first = tokio::spawn(async move {
        first_source
            .pull("repldb", "first", 0, 10, Some(WAIT_MS))
            .await
            .expect("first long-poll pull")
    });
    let second_source = source.clone();
    let second = tokio::spawn(async move {
        second_source
            .pull("repldb", "second", 0, 10, Some(WAIT_MS))
            .await
            .expect("second long-poll pull")
    });
    let (first, second) = tokio::time::timeout(Duration::from_secs(30), async {
        tokio::join!(first, second)
    })
    .await
    .expect("both long polls return within their budget");
    assert!(matches!(
        first.expect("first pull task"),
        ReplResponse::Pull { .. }
    ));
    assert!(matches!(
        second.expect("second pull task"),
        ReplResponse::Pull { .. }
    ));
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_millis(u64::from(WAIT_MS) * 8 / 5),
        "pulls of different targets must overlap, not serialize behind one          source lock: both took {elapsed:?} for a {WAIT_MS} ms wait each"
    );

    handle.shutdown().await;
}
