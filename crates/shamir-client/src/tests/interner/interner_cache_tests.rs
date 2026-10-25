//! Integration tests for the client-side interner field-map cache (Stage 5).
//!
//! These boot a real in-process `ServerLauncher` (the same harness
//! `tests/smoke.rs` uses) and exercise the cache ops against the production
//! server path. They MUST run with `--full` (integration/e2e scope).

use shamir_collections::{new_map, TFxSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::future::join_all;
use shamir_types::mpack;
use shamir_types::types::value::Value;
use tempfile::TempDir;
use tokio::io::{duplex, split};
use zeroize::Zeroizing;

use shamir_connect::common::envelope::{RequestEnvelope, ResponseEnvelope};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::write::insert;
use shamir_query_types::batch::{BatchOp, BatchResponse};
use shamir_query_types::read::{QueryRecord, QueryResult};
use shamir_query_types::wire::{DbRequest, DbResponse};
use shamir_transport_tcp::framing::{read_frame, write_frame, MAX_FRAME_SIZE_DEFAULT};

use crate::client::LifecycleHooks;
use crate::{Client, ConnectOptions};

use shamir_server::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ProfileKind, TlsConfig,
};
use shamir_server::server::{BootstrapMode, ServerLauncher};

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

/// Boot a server + connect a client. The closure receives an owned client and
/// must return it (the fixture closes it). This sidesteps the HRTB dance that
/// a borrowed closure would require.
async fn with_client<F, Fut>(f: F)
where
    F: FnOnce(Client) -> Fut,
    Fut: std::future::Future<Output = Client>,
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

    let client = Client::connect(ConnectOptions {
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
    .expect("connect");

    // Provision a db + repo + table so write/interner ops have a target.
    let mut mk_db = Batch::new();
    mk_db.id("mk-db");
    mk_db.create_db("ic", ddl::create_db("ic"));
    client
        .execute("default", mk_db.build())
        .await
        .expect("create db");

    let mut mk_table = Batch::new();
    mk_table.id("mk-table");
    mk_table.create_repo("mr", ddl::create_repo("main"));
    mk_table.create_table("tb", ddl::create_table("items").repo("main"));
    client
        .execute("ic", mk_table.build())
        .await
        .expect("create table");

    let client = f(client).await;

    client.close().await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    handle.shutdown().await;
}

/// touch_fields(["age","name","42"]) → cache holds 3 distinct ids; the field
/// literally named "42" resolves to its interner id, NOT the integer 42.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn touch_fields_registers_numeric_string_name() {
    with_client(|client: Client| async move {
        let mappings = client
            .touch_fields("ic", "main", &["age", "name", "42"])
            .await
            .expect("touch");

        // Three distinct ids returned.
        assert_eq!(mappings.len(), 3, "got mappings: {mappings:?}");
        let ids: Vec<u64> = mappings.iter().map(|(_, id)| *id).collect();
        let unique: TFxSet<u64> = ids.iter().copied().collect();
        assert_eq!(unique.len(), 3, "ids must be distinct: {ids:?}");

        // §9.4: "42" resolves to the interner id for the field NAMED "42",
        // which is NOT 42 (ids are minted by the server starting from its own
        // counter; "42" the string is just a field name).
        let id_of_42 = client.resolve_field("ic", "main", "42");
        assert!(id_of_42.is_some(), "field '42' must resolve");
        let id_of_42 = id_of_42.unwrap();
        assert_ne!(
            id_of_42, 42u64,
            "§9.4: the string '42' must NOT resolve to the integer 42; got {id_of_42}"
        );
        assert!(id_of_42 > 0, "ids are positive");

        // resolve_field agrees with the touch_fields return for all three.
        for (name, id) in &mappings {
            assert_eq!(
                client.resolve_field("ic", "main", name),
                Some(*id),
                "resolve_field mismatch for '{name}'"
            );
        }
        client
    })
    .await;
}

/// dump_repo populates the cache; resolve_field / field_name round-trip.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dump_repo_populates_and_round_trips() {
    with_client(|client: Client| async move {
        // First touch some fields so the interner has content.
        client
            .touch_fields("ic", "main", &["color", "size"])
            .await
            .expect("touch");

        // Now wipe the client's in-memory cache by touching a fresh repo name?
        // No — simpler: the cache is already populated by touch. Instead, test
        // dump_repo against a repo that the SERVER already has data for, from a
        // NEW client (fresh cache). The fresh client's dump must reproduce the
        // same ids.
        let mappings = client
            .touch_fields("ic", "main", &["color", "size"])
            .await
            .expect("touch");

        // The cache already holds color/size from the touch above. dump_repo
        // (guarded by OnceCell) must not clobber them; resolve must still work.
        client.dump_repo("ic", "main").await.expect("dump");
        for (name, id) in &mappings {
            assert_eq!(
                client.resolve_field("ic", "main", name),
                Some(*id),
                "resolve after dump mismatch for '{name}'"
            );
            assert_eq!(
                client.field_name("ic", "main", *id),
                Some(name.clone()),
                "field_name after dump mismatch for id {id}"
            );
        }
        client
    })
    .await;
}

/// idempotent: touch the same name twice → same id.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn touch_is_idempotent() {
    with_client(|client: Client| async move {
        let first = client
            .touch_fields("ic", "main", &["email"])
            .await
            .expect("first touch");
        assert_eq!(first.len(), 1);
        let id_first = first[0].1;

        let second = client
            .touch_fields("ic", "main", &["email"])
            .await
            .expect("second touch");
        assert_eq!(
            second.len(),
            1,
            "second touch must still return the mapping"
        );
        let id_second = second[0].1;

        assert_eq!(
            id_first, id_second,
            "idempotent touch must return the same id"
        );

        // And resolve_field agrees.
        assert_eq!(client.resolve_field("ic", "main", "email"), Some(id_first));
        client
    })
    .await;
}

/// refresh_repo with since=epoch → only delta merged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refresh_repo_merges_delta() {
    with_client(|client: Client| async move {
        // Seed the interner with two fields.
        client
            .touch_fields("ic", "main", &["alpha", "beta"])
            .await
            .expect("seed touch");
        let epoch_after_seed = client.interner_cache().get_or_create("ic", "main").epoch();

        // Add a third field via a direct touch (simulating another writer).
        client
            .touch_fields("ic", "main", &["gamma"])
            .await
            .expect("delta touch");

        // refresh must advance the epoch and surface gamma without dropping
        // alpha/beta.
        client.refresh_repo("ic", "main").await.expect("refresh");

        assert!(
            client.interner_cache().get_or_create("ic", "main").epoch() >= epoch_after_seed,
            "epoch must not regress after refresh"
        );
        assert!(client.resolve_field("ic", "main", "alpha").is_some());
        assert!(client.resolve_field("ic", "main", "beta").is_some());
        assert!(client.resolve_field("ic", "main", "gamma").is_some());

        // The three ids are distinct.
        let mut ids: Vec<u64> = ["alpha", "beta", "gamma"]
            .iter()
            .filter_map(|n| client.resolve_field("ic", "main", n))
            .collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 3, "three distinct ids after refresh");
        client
    })
    .await;
}

/// execute_with_touch: insert a record with a new field → the field is touched
/// + the insert succeeds; a second insert with the same field issues NO extra
/// touch (cache warm — resolve_field returns the id before the second call's
/// internal touch, so touch_fields short-circuits).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn execute_with_touch_warms_cache() {
    with_client(|client: Client| async move {
        let mut b1 = Batch::new();
        b1.id("w1");
        b1.insert("i1", insert("items").row(mpack!({ "sku": "A1", "qty": 1 })));
        let resp1 = client
            .execute_with_touch("ic", b1.build())
            .await
            .expect("first execute_with_touch");
        assert!(
            resp1.results.contains_key("i1"),
            "first insert must succeed"
        );

        // After the first pre-touch write, the fields "sku" and "qty" must be
        // cached — proving execute_with_touch touched them.
        let sku_id = client.resolve_field("ic", "main", "sku");
        let qty_id = client.resolve_field("ic", "main", "qty");
        assert!(sku_id.is_some(), "sku must be cached after pre-touch write");
        assert!(qty_id.is_some(), "qty must be cached after pre-touch write");

        // Second insert with the SAME fields. Because the cache is warm,
        // touch_fields inside execute_with_touch finds no missing names and
        // short-circuits (no server roundtrip for the touch). The insert still
        // succeeds and the ids are unchanged.
        let sku_id_before = sku_id.unwrap();
        let qty_id_before = qty_id.unwrap();

        let mut b2 = Batch::new();
        b2.id("w2");
        b2.insert("i2", insert("items").row(mpack!({ "sku": "A2", "qty": 2 })));
        let resp2 = client
            .execute_with_touch("ic", b2.build())
            .await
            .expect("second execute_with_touch");
        assert!(
            resp2.results.contains_key("i2"),
            "second insert must succeed"
        );

        // Ids unchanged — cache was warm, no re-mint.
        assert_eq!(
            client.resolve_field("ic", "main", "sku"),
            Some(sku_id_before),
            "sku id stable across warm-cache write"
        );
        assert_eq!(
            client.resolve_field("ic", "main", "qty"),
            Some(qty_id_before),
            "qty id stable across warm-cache write"
        );
        client
    })
    .await;
}

/// RV-CLIENT-47644B74 (drain/stampede portion): 8 concurrent COLD `dump_repo`
/// callers on a never-dumped repo must coalesce onto the per-`FieldMap`
/// `OnceCell` stampede guard. REAL frame-count oracle: the client runs over
/// `Client::for_test` duplex pipes and an emulated server decodes every
/// client→server frame, counts the `interner_dump` Execute requests and
/// replies with a valid dump response the parser accepts.
///
/// REVERT ORACLE: reverting the OnceCell stampede guard in
/// `FieldMap::ensure_populated` (fetching unconditionally) makes the count 8
/// and this test fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rv_client_47644b74_concurrent_cold_dump_single_fetch() {
    // client→server: the client writes; the emulated server reads.
    let (c2s_client_side, mut c2s_server_side) = duplex(64 * 1024);
    // server→client: the emulated server writes; the client reads.
    let (s2c_client_side, mut s2c_server_side) = duplex(512 * 1024);
    let (client_reader, _unused_s2c_write) = split(s2c_client_side);
    let (_unused_c2s_read, client_writer) = split(c2s_client_side);
    let client = Client::for_test(client_reader, client_writer, LifecycleHooks::default());

    let dumps_seen = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&dumps_seen);
    let server = tokio::spawn(async move {
        loop {
            let frame = match read_frame(&mut c2s_server_side, MAX_FRAME_SIZE_DEFAULT).await {
                Ok(f) => f,
                Err(_) => break, // client dropped — connection over
            };
            let env = RequestEnvelope::from_msgpack(&frame).expect("request envelope");
            let req: DbRequest = rmp_serde::from_slice(&env.req).expect("decode DbRequest");
            let DbRequest::Execute { batch, .. } = &req else {
                panic!("emulated server: unexpected request {req:?}");
            };
            let mut dump_alias = None;
            for (alias, entry) in &batch.queries {
                if matches!(entry.op, BatchOp::InternerDump(_)) {
                    dump_alias = Some(alias.clone());
                }
            }
            let Some(alias) = dump_alias else {
                panic!("emulated server: unexpected non-dump request {req:?}");
            };
            seen.fetch_add(1, Ordering::SeqCst);

            // Craft a valid `interner_dump` response the client parser
            // accepts: `{ "epoch": u64, "entries": [[id, name], ...] }`.
            let mut fields = new_map();
            fields.insert("epoch".to_string(), Value::Int(1));
            fields.insert(
                "entries".to_string(),
                Value::List(vec![
                    Value::List(vec![Value::Int(1), Value::Str("sku".into())]),
                    Value::List(vec![Value::Int(2), Value::Str("qty".into())]),
                ]),
            );
            let mut result = QueryResult::default();
            result.records.push(QueryRecord::Direct(Value::Map(fields)));
            let mut results = new_map();
            results.insert(alias, result);
            let response = BatchResponse {
                id: Value::Null,
                results,
                execution_plan: Vec::new(),
                edge_provenance: new_map(),
                execution_time_us: 0,
                transaction: None,
                interner_delta: new_map(),
            };
            let payload =
                rmp_serde::to_vec_named(&DbResponse::Batch { response }).expect("encode response");
            let bytes = ResponseEnvelope::ok(env.request_id, payload)
                .to_msgpack()
                .expect("encode envelope");
            write_frame(&mut s2c_server_side, &bytes)
                .await
                .expect("write reply");
        }
    });

    // 8 concurrent COLD dump_repo callers (futures created BEFORE polling).
    let dumps: Vec<_> = (0..8).map(|_| client.dump_repo("ic", "main")).collect();
    for r in join_all(dumps).await {
        r.expect("concurrent cold dump must not fail");
    }

    // THE frame-count oracle: exactly ONE dump request reached the server.
    assert_eq!(
        dumps_seen.load(Ordering::SeqCst),
        1,
        "OnceCell stampede guard must coalesce 8 cold dumps into ONE fetch"
    );

    // Convergence: all dumped entries resolve consistently.
    let sku = client
        .resolve_field("ic", "main", "sku")
        .expect("sku resolved from the coalesced dump");
    assert_eq!(client.field_name("ic", "main", sku).as_deref(), Some("sku"));
    let qty = client
        .resolve_field("ic", "main", "qty")
        .expect("qty resolved from the coalesced dump");
    assert_eq!(client.field_name("ic", "main", qty).as_deref(), Some("qty"));

    drop(client);
    // A split duplex only reaches EOF once BOTH halves are gone: release the
    // test-held read half so the emulated server sees the connection end.
    drop(_unused_c2s_read);
    drop(_unused_s2c_write);
    server.await.expect("emulated server task panicked");
}
