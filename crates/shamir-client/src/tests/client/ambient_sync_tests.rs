//! Ambient interner epoch-delta sync tests (Stage 5-wire Part A).
//!
//! These boot a real in-process server with TWO clients (each with its own
//! `InternerCacheRegistry`). Client A touches a new field → server mints an
//! id; client B's subsequent request carries its (epoch=0) → the response's
//! `interner_delta` carries the new (id,name) → B's cache resolves the field
//! WITHOUT an explicit dump.
//!
//! Must run with `--full` (integration/e2e scope).

use std::path::PathBuf;
use std::time::Duration;

use tempfile::TempDir;
use zeroize::Zeroizing;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;

use shamir_connect::common::envelope::RequestEnvelope;
use shamir_query_types::wire::DbRequest;
use tokio::io::AsyncReadExt;

use crate::{Client, ConnectOptions};

use shamir_server::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ProfileKind, TlsConfig,
};
use shamir_server::server::{BootstrapMode, ServerLauncher};

/// Fast Argon2id profile so the SCRAM handshake completes quickly.
fn fast_kdf() -> KdfConfig {
    KdfConfig {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

fn make_config(temp: &TempDir) -> Config {
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
        kdf_defaults: fast_kdf(),
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
        observability: shamir_server::config::ObservabilityConfig {
            addr: String::new(),
            allow_public_metrics: false,
        },
        replication: None,
    }
}

/// Boot a server + connect TWO clients (each with an independent cache).
/// The closure receives both clients and must return them.
async fn with_two_clients<F, Fut>(f: F)
where
    F: FnOnce(Client, Client) -> Fut,
    Fut: std::future::Future<Output = (Client, Client)>,
{
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().expect("tempdir");
    let password = b"correct horse battery staple".to_vec();

    let launcher = ServerLauncher {
        config: make_config(&temp),
        bootstrap: BootstrapMode::Password {
            username: "admin".into(),
            password: Zeroizing::new(password.clone()),
        },
    };
    let handle = launcher.launch().await.expect("launcher boot");
    let addr = handle.first_tls_exporter_addr().expect("bound");

    let connect = || {
        let password = password.clone();
        async move {
            Client::connect(ConnectOptions {
                addr,
                server_name: "localhost".into(),
                username: "admin".into(),
                password: Zeroizing::new(password),
                accept_new_host: true,
                trusted_pin: None,
                connect_timeout: None,
                handshake_timeout: None,
                request_timeout: None,
            })
            .await
            .expect("connect")
        }
    };

    let client_a = connect().await;
    let client_b = connect().await;

    // Provision a db + repo + table so write/interner ops have a target.
    let mut mk_db = Batch::new();
    mk_db.id("mk-db");
    mk_db.create_db("ic", ddl::create_db("ic"));
    client_a
        .execute("default", mk_db.build())
        .await
        .expect("create db");

    let mut mk_table = Batch::new();
    mk_table.id("mk-table");
    mk_table.create_repo("mr", ddl::create_repo("main"));
    mk_table.create_table("tb", ddl::create_table("items").repo("main"));
    client_a
        .execute("ic", mk_table.build())
        .await
        .expect("create table");

    let (client_a, client_b) = f(client_a, client_b).await;

    client_a.close().await;
    client_b.close().await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    handle.shutdown().await;
}

#[tokio::test]
async fn rv_client_1c219006_default_client_version_is_legacy() {
    let (s2c, _unused) = tokio::io::duplex(1024);
    let (reader, _write) = tokio::io::split(s2c);
    let (_read, writer) = tokio::io::split(tokio::io::duplex(1024).0);
    let client = Client::for_test(reader, writer, crate::client::LifecycleHooks::default());
    assert_eq!(client.server_query_version_for_test(), 0, "legacy default");
    drop(client);
}

#[tokio::test]
async fn rv_client_1c219006_v1_execution_does_not_create_cache_keys() {
    // Check the encoded request, not only the admission predicate.
    let (client_to_server, mut server_reader) = tokio::io::duplex(64 * 1024);
    let (_server_to_client, client_reader) = tokio::io::duplex(64 * 1024);
    let mut batch = Batch::new();
    batch.query("read", shamir_query_builder::Query::from("items"));
    let client = Client::for_test(
        client_reader,
        client_to_server,
        crate::client::LifecycleHooks::default(),
    );
    let execute = tokio::spawn(async move { client.execute("ic", batch.build()).await });
    let mut prefix = [0u8; 4];
    server_reader
        .read_exact(&mut prefix)
        .await
        .expect("frame prefix");
    let len = u32::from_be_bytes(prefix) as usize;
    assert!(len <= 64 * 1024, "bounded test frame");
    let mut frame = vec![0; len];
    server_reader
        .read_exact(&mut frame)
        .await
        .expect("frame body");
    let envelope = RequestEnvelope::from_msgpack(&frame).expect("request envelope");
    let DbRequest::Execute { batch, .. } =
        rmp_serde::from_slice(&envelope.req).expect("execute request")
    else {
        panic!("expected Execute");
    };
    assert!(
        batch.interner_epochs.is_empty(),
        "legacy execution sends no cursor"
    );
    execute.abort();
    let _ = execute.await;
}

#[tokio::test]
async fn rv_client_1c219006_v2_execution_advertises_cold_epoch() {
    // Decode the actual outbound frame to protect cold-v2 synchronization.
    let (client_to_server, mut server_reader) = tokio::io::duplex(64 * 1024);
    let (_server_to_client, client_reader) = tokio::io::duplex(64 * 1024);
    let mut batch = Batch::new();
    batch.query("read", shamir_query_builder::Query::from("items"));
    let client = Client::for_test(
        client_reader,
        client_to_server,
        crate::client::LifecycleHooks::default(),
    );
    client.set_server_query_version_for_test(2);
    let execute = tokio::spawn(async move { client.execute("ic", batch.build()).await });
    let mut prefix = [0u8; 4];
    server_reader
        .read_exact(&mut prefix)
        .await
        .expect("frame prefix");
    let len = u32::from_be_bytes(prefix) as usize;
    assert!(len <= 64 * 1024, "bounded frame from client-under-test");
    let mut frame = vec![0; len];
    server_reader
        .read_exact(&mut frame)
        .await
        .expect("frame body");
    let envelope = RequestEnvelope::from_msgpack(&frame).expect("request envelope");
    let DbRequest::Execute { batch, .. } =
        rmp_serde::from_slice(&envelope.req).expect("execute request")
    else {
        panic!("expected Execute");
    };
    assert_eq!(batch.interner_epochs.len(), 1, "one distinct repo cursor");
    assert_eq!(batch.interner_epochs.values().next(), Some(&0));
    execute.abort();
    let _ = execute.await;
}

/// Core ambient sync test: A touches a new field → B learns from its next
/// request's response delta WITHOUT an explicit dump.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ambient_delta_client_b_learns_without_dump() {
    with_two_clients(|client_a: Client, client_b: Client| async move {
        // Client A touches "alpha" on repo "main" — server mints id.
        let mappings_a = client_a
            .touch_fields("ic", "main", &["alpha"])
            .await
            .expect("A touch alpha");
        assert_eq!(mappings_a.len(), 1);
        let alpha_id = mappings_a[0].1;
        assert!(alpha_id > 0);

        // Client B has a fresh cache (epoch=0) and does NOT know "alpha".
        assert!(
            client_b.resolve_field("ic", "main", "alpha").is_none(),
            "B must not know 'alpha' before its request"
        );

        // Client B issues a normal READ on repo "main". Its execute() populates
        // interner_epochs[main]=0; the server attaches the delta; B's cache
        // merges it — WITHOUT an explicit dump.
        let mut read_batch = Batch::new();
        read_batch.id("b-read");
        read_batch.query("r", shamir_query_builder::Query::from("items"));
        let resp = client_b
            .execute("ic", read_batch.build())
            .await
            .expect("B read");

        // B's cache must now resolve "alpha" to the SAME id A got.
        let b_alpha = client_b.resolve_field("ic", "main", "alpha");
        assert_eq!(
            b_alpha,
            Some(alpha_id),
            "B must learn 'alpha'={alpha_id} via ambient delta, got {b_alpha:?}"
        );

        // The response must have carried the delta.
        assert!(
            resp.interner_delta.contains_key("main"),
            "response must carry interner_delta for 'main'"
        );

        (client_a, client_b)
    })
    .await;
}

/// Empty delta when B is already up-to-date: after B learns via ambient sync,
/// a second request to the same repo produces no new entries (B's epoch
/// matches the server's high-water).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ambient_delta_empty_when_up_to_date() {
    with_two_clients(|client_a: Client, client_b: Client| async move {
        // A touches two fields.
        client_a
            .touch_fields("ic", "main", &["beta", "gamma"])
            .await
            .expect("A touch");

        // B learns via ambient sync (first read).
        let mut read1 = Batch::new();
        read1.id("b1");
        read1.query("r", shamir_query_builder::Query::from("items"));
        client_b
            .execute("ic", read1.build())
            .await
            .expect("B first read");

        assert!(client_b.resolve_field("ic", "main", "beta").is_some());
        assert!(client_b.resolve_field("ic", "main", "gamma").is_some());

        // Second read: B is now up-to-date → delta entries should be empty.
        let mut read2 = Batch::new();
        read2.id("b2");
        read2.query("r", shamir_query_builder::Query::from("items"));
        let resp2 = client_b
            .execute("ic", read2.build())
            .await
            .expect("B second read");

        if let Some(delta) = resp2.interner_delta.get("main") {
            assert!(
                delta.entries.is_empty(),
                "delta entries must be empty when B is up-to-date, got {:?}",
                delta.entries
            );
        }
        // If the key is absent entirely that's also fine (server may omit
        // empty deltas depending on entries_after behaviour).

        (client_a, client_b)
    })
    .await;
}

/// Epoch advances (CAS-max): after ambient learning, B's epoch must be ≥ the
/// server's high-water mark. A subsequent touch of a NEW field by A, followed
/// by B's read, must surface only the new entry (delta).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ambient_delta_epoch_advances_cas_max() {
    with_two_clients(|client_a: Client, client_b: Client| async move {
        // A touches delta_epoch_advances_cas_max_1.
        client_a
            .touch_fields("ic", "main", &["deac_1"])
            .await
            .expect("A touch deac_1");

        // B learns.
        let mut read1 = Batch::new();
        read1.id("b1");
        read1.query("r", shamir_query_builder::Query::from("items"));
        client_b
            .execute("ic", read1.build())
            .await
            .expect("B read 1");
        let epoch_after_first = client_b
            .interner_cache()
            .get_or_create("ic", "main")
            .epoch();
        assert!(
            epoch_after_first > 0,
            "B epoch must advance past 0 after learning"
        );

        // A touches a NEW field.
        client_a
            .touch_fields("ic", "main", &["deac_2"])
            .await
            .expect("A touch deac_2");

        // B reads again — must learn deac_2 (delta) and epoch must not regress.
        let mut read2 = Batch::new();
        read2.id("b2");
        read2.query("r", shamir_query_builder::Query::from("items"));
        client_b
            .execute("ic", read2.build())
            .await
            .expect("B read 2");

        assert!(
            client_b.resolve_field("ic", "main", "deac_2").is_some(),
            "B must learn deac_2 via the second ambient delta"
        );
        let epoch_after_second = client_b
            .interner_cache()
            .get_or_create("ic", "main")
            .epoch();
        assert!(
            epoch_after_second >= epoch_after_first,
            "B epoch must not regress: was {epoch_after_first}, now {epoch_after_second}"
        );

        (client_a, client_b)
    })
    .await;
}
