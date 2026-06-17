//! Integration tests for the v2 MessagePack pass-through (S-client).
//!
//! Tests boot a real in-process `ServerLauncher` and exercise the full
//! id-keyed write + read path via `execute_with_touch`. They MUST run with
//! `--full` (integration/e2e scope) since they require a live server.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::json;
use tempfile::TempDir;
use zeroize::Zeroizing;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::query::Query;
use shamir_query_builder::write::insert;

use crate::{Client, ConnectOptions};

use shamir_server::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ProfileKind, TlsConfig,
};
use shamir_server::server::{BootstrapMode, ServerHandle, ServerLauncher};

/// Fast Argon2id profile so the SCRAM handshake completes in ~50 ms not ~2 s.
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
        },
    }
}

/// Boot a server + connect a client. Returns `(handle, addr, password)` so
/// tests can create additional clients against the same server.
async fn boot_server(temp: &TempDir) -> (ServerHandle, std::net::SocketAddr, Vec<u8>) {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let password = b"correct horse battery staple".to_vec();
    let launcher = ServerLauncher {
        config: make_config(temp),
        bootstrap: BootstrapMode::Password {
            username: "admin".into(),
            password: Zeroizing::new(password.clone()),
        },
    };
    let handle = launcher.launch().await.expect("launcher boot");
    let addr = handle.first_tls_exporter_addr().expect("bound");
    (handle, addr, password)
}

async fn connect_client(addr: std::net::SocketAddr, password: Vec<u8>) -> Client {
    Client::connect(ConnectOptions {
        addr,
        server_name: "localhost".into(),
        username: "admin".into(),
        password: Zeroizing::new(password),
        accept_new_host: true,
        trusted_pin: None,
    })
    .await
    .expect("connect")
}

/// Provision a fresh db + repo + table on the server.
async fn provision_db_table(client: &Client, db: &str) {
    let mut mk_db = Batch::new();
    mk_db.id("mk-db");
    mk_db.create_db(db, ddl::create_db(db));
    client
        .execute("default", mk_db.build())
        .await
        .expect("create db");

    let mut mk_table = Batch::new();
    mk_table.id("mk-table");
    mk_table.create_repo("mr", ddl::create_repo("main"));
    mk_table.create_table("tb", ddl::create_table("items").repo("main"));
    client
        .execute(db, mk_table.build())
        .await
        .expect("create table");
}

/// Boot a server + connect a single client. The closure receives an owned
/// client and must return it (the fixture closes it). This sidesteps the HRTB
/// dance that a borrowed closure would require.
async fn with_client<F, Fut>(f: F)
where
    F: FnOnce(Client) -> Fut,
    Fut: std::future::Future<Output = Client>,
{
    let temp = TempDir::new().expect("tempdir");
    let (handle, addr, password) = boot_server(&temp).await;
    let client = connect_client(addr, password).await;
    provision_db_table(&client, "v2db").await;
    let client = f(client).await;
    client.close().await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    handle.shutdown().await;
}

// ─── Tests ────────────────────────────────────────────────────────────────────

/// E2E round-trip: a v2 client INSERTs a record id-keyed (via records_idmsgpack)
/// and READs it back (id-keyed, de-interned client-side). The read result must
/// equal the inserted fields.
///
/// This exercises the WHOLE pass-through chain live:
///   client pre-touch → id-keyed encode → server verbatim store →
///   server id-keyed read → client de-intern → name-keyed QueryValue
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v2_e2e_roundtrip_insert_read() {
    with_client(|client: Client| async move {
        // Skip if server is not v2 — the test exercises the new code path.
        if client.server_query_version() < 2 {
            return client;
        }

        // INSERT via execute_with_touch (v2 path: encodes id-keyed internally).
        let mut b = Batch::new();
        b.id("w1");
        b.insert(
            "ins",
            insert("items").row(json!({
                "name": "widget",
                "price": 9,
                "active": true
            })),
        );
        let ins_resp = client
            .execute_with_touch("v2db", b.build())
            .await
            .expect("insert");
        assert!(
            ins_resp.results.contains_key("ins"),
            "insert op must return a result"
        );

        // READ back via execute_with_touch (sets result_encoding=Id on v2).
        let mut rb = Batch::new();
        rb.id("r1");
        rb.query("sel", Query::from("items"));
        let read_resp = client
            .execute_with_touch("v2db", rb.build())
            .await
            .expect("read");

        let result = read_resp.results.get("sel").expect("sel result");
        assert!(
            !result.records.is_empty(),
            "read must return at least one record"
        );

        // The record must have come back as name-keyed (de-interned by client).
        // as_json() works on both Direct and Json variants.
        let rec_json = result.records[0].as_json();
        assert_eq!(
            rec_json.get("name").and_then(|v| v.as_str()),
            Some("widget"),
            "name field must round-trip"
        );
        assert_eq!(
            rec_json.get("price").and_then(|v| v.as_i64()),
            Some(9),
            "price field must round-trip"
        );
        assert_eq!(
            rec_json.get("active").and_then(|v| v.as_bool()),
            Some(true),
            "active field must round-trip"
        );

        client
    })
    .await;
}

/// §9.4: a field literally named "42" must round-trip as the STRING key "42",
/// not as the integer 42. The server assigns a real interner id to this field
/// name (which will be something like 1, 2, 3 … NOT 42). Client-side de-intern
/// must produce the STRING key "42".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v2_section_9_4_numeric_string_field_name() {
    with_client(|client: Client| async move {
        if client.server_query_version() < 2 {
            return client;
        }

        // Insert a record with field name "42".
        let mut b = Batch::new();
        b.id("ins-42");
        b.insert(
            "ins",
            insert("items").row(json!({ "42": "numeric-name-value" })),
        );
        client
            .execute_with_touch("v2db", b.build())
            .await
            .expect("insert");

        // Read back and check the field is present under key "42" (string).
        let mut rb = Batch::new();
        rb.id("r-42");
        rb.query("sel", Query::from("items"));
        let read_resp = client
            .execute_with_touch("v2db", rb.build())
            .await
            .expect("read");

        let result = read_resp.results.get("sel").expect("sel result");
        assert!(
            !result.records.is_empty(),
            "read must return at least one record"
        );

        // Verify the field named "42" round-trips correctly as a string key.
        let rec_json = result.records[0].as_json();
        let val = rec_json.get("42");
        assert!(
            val.is_some(),
            "§9.4: field named '42' must be present in the response as string key '42'"
        );
        assert_eq!(
            val.and_then(|v| v.as_str()),
            Some("numeric-name-value"),
            "§9.4: field value must round-trip"
        );

        // Also verify: the interner id for "42" is NOT 42 itself.
        // §9.4: the string "42" is a field NAME; its server-assigned id will be
        // some monotonically-minted small integer (1, 2, ...) not 42.
        let id_of_42 = client.resolve_field("v2db", "main", "42");
        assert!(id_of_42.is_some(), "field '42' must be in the cache");
        assert_ne!(
            id_of_42.unwrap(),
            42u64,
            "§9.4: the interner id for field named '42' must NOT be 42"
        );

        client
    })
    .await;
}

/// New-field test: inserting a record with a brand-new field name triggers
/// `touch_fields` to mint the field on the server. The id-keyed encode then
/// succeeds, and the field appears in the read-back result.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v2_new_field_pre_touch_and_roundtrip() {
    with_client(|client: Client| async move {
        if client.server_query_version() < 2 {
            return client;
        }

        // Use a field name that has NOT been touched before in this fresh server.
        let unique_field = "brand_new_field_xyz";

        // Verify the field is NOT in the cache yet.
        assert!(
            client.resolve_field("v2db", "main", unique_field).is_none(),
            "new field must not be cached before the first execute_with_touch"
        );

        // Insert a record with the new field — execute_with_touch will pre-touch
        // and mint the field on the server.
        let mut b = Batch::new();
        b.id("new-f");
        b.insert(
            "ins",
            insert("items").row(json!({ unique_field: "hello-new-field" })),
        );
        client
            .execute_with_touch("v2db", b.build())
            .await
            .expect("insert with new field");

        // The field must now be in the cache.
        assert!(
            client.resolve_field("v2db", "main", unique_field).is_some(),
            "new field must be cached after execute_with_touch"
        );

        // Read back and verify the field is present in at least one record.
        let mut rb = Batch::new();
        rb.id("r-new");
        rb.query("sel", Query::from("items"));
        let read_resp = client
            .execute_with_touch("v2db", rb.build())
            .await
            .expect("read");
        let result = read_resp.results.get("sel").expect("sel result");

        let found = result
            .records
            .iter()
            .any(|rec| rec.as_json().get(unique_field).is_some());
        assert!(found, "new field must appear in at least one read record");

        client
    })
    .await;
}

/// De-intern unknown-id refresh test: a second client with a COLD cache reads
/// rows whose ids it has never seen. The `deintern_id_bytes` path calls
/// `refresh_repo` to recover the names, and the result is correctly de-interned.
///
/// Setup:
/// 1. Client A inserts a record with a new field ("refresh_field") via `execute_with_touch`.
/// 2. Client B connects to the SAME server with a fresh (empty) interner cache.
/// 3. Client B calls `execute_with_touch` for a SELECT (read-only); on v2 this
///    sets `result_encoding = Id`. The response contains IdBytes rows with ids
///    that Client B has never seen.
/// 4. `deintern_id_bytes` fails attempt-1, calls `refresh_repo`, merges the
///    delta, succeeds on attempt-2. Client B returns name-keyed records.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v2_deintern_unknown_id_refresh() {
    let temp = TempDir::new().expect("tempdir");
    let (handle, addr, password) = boot_server(&temp).await;

    // Client A: mint a field and insert a record.
    let client_a = connect_client(addr, password.clone()).await;
    provision_db_table(&client_a, "rfdb").await;

    if client_a.server_query_version() < 2 {
        client_a.close().await;
        handle.shutdown().await;
        return;
    }

    {
        let mut b = Batch::new();
        b.id("ins-rf");
        b.insert(
            "ins",
            insert("items").row(json!({ "refresh_field": "refresh-value" })),
        );
        client_a
            .execute_with_touch("rfdb", b.build())
            .await
            .expect("client_a insert");
    }

    // Verify client A has the field cached.
    assert!(
        client_a
            .resolve_field("rfdb", "main", "refresh_field")
            .is_some(),
        "client A must have 'refresh_field' cached"
    );

    client_a.close().await;
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Client B: fresh connection, EMPTY interner cache.
    let client_b = connect_client(addr, password).await;

    // Sanity: client B cache is cold for "refresh_field".
    assert!(
        client_b
            .resolve_field("rfdb", "main", "refresh_field")
            .is_none(),
        "client B must start with empty cache"
    );

    // Client B does a read-only execute_with_touch. The response is id-keyed
    // (result_encoding = Id on v2). deintern_id_bytes will not find the ids
    // in the empty cache, call refresh_repo, and succeed on retry.
    let mut rb = Batch::new();
    rb.id("r-rf");
    rb.query("sel", Query::from("items"));
    let read_resp = client_b
        .execute_with_touch("rfdb", rb.build())
        .await
        .expect("client_b read with cold cache");

    let result = read_resp.results.get("sel").expect("sel result");
    assert!(
        !result.records.is_empty(),
        "read must return at least one record"
    );

    // After the refresh path, "refresh_field" must be present in the result
    // as a name-keyed string field.
    let found = result
        .records
        .iter()
        .any(|rec| rec.as_json().get("refresh_field").is_some());
    assert!(
        found,
        "after de-intern via refresh_repo, 'refresh_field' must appear in a record"
    );

    // After deintern succeeded, the field must now be in Client B's cache too
    // (because refresh_repo merged it).
    assert!(
        client_b
            .resolve_field("rfdb", "main", "refresh_field")
            .is_some(),
        "client B cache must be populated after refresh_repo"
    );

    client_b.close().await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    handle.shutdown().await;
}
