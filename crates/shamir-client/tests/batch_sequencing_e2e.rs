//! End-to-end proof of OQL Epic 01 sequencing (Phases A/B) over a REAL wire
//! round-trip: real `ServerLauncher` (TCP), real `shamir_client::Client`,
//! real SCRAM handshake — not an in-process planner call.
//!
//! Mirrors the boot/connect pattern of `smoke.rs` but focuses on the
//! `edge_provenance` field (`EdgeKind::Explicit` / `DataFlow` / `Both`) and
//! the Phase A guarantee that a pure-`after` edge is ordering-only and does
//! NOT open a `$query` data channel.
//!
//! Task #631 — OQL Epic 01 / Phase D.

use std::path::PathBuf;

use tempfile::TempDir;
use zeroize::Zeroizing;

use shamir_client::builder::batch::Batch;
use shamir_client::builder::ddl;
use shamir_client::builder::doc;
use shamir_client::builder::write::{insert, update};
use shamir_client::builder::Query;
use shamir_client::{Client, ClientError, ConnectOptions};
use shamir_query_types::batch::EdgeKind;

use shamir_server::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ProfileKind, TlsConfig,
};
use shamir_server::server::{BootstrapMode, ServerLauncher};

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
            allow_public_metrics: false,
        },
        replication: None,
    }
}

/// Boots a real server + connects a real client. Returns both so the caller
/// can run batches and then shut down.
async fn boot() -> (
    shamir_server::server::ServerHandle,
    Client,
    Zeroizing<Vec<u8>>,
) {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().expect("tempdir");
    // Leak the tempdir so it outlives this fn — the server keeps using the
    // data_dir for the lifetime of the test.
    let temp = Box::leak(Box::new(temp));
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

    let client = Client::connect(ConnectOptions {
        addr,
        server_name: "localhost".into(),
        username: "admin".into(),
        password: Zeroizing::new(password.clone()),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: None,
        request_timeout: None,
    })
    .await
    .expect("connect");

    (handle, client, Zeroizing::new(password))
}

/// Creates db `seqdb` with repo `main` / table `items`, ready for DML.
async fn make_db(client: &Client) {
    let mut mk_db = Batch::new();
    mk_db.id("mk-db");
    mk_db.create_db("mk", ddl::create_db("seqdb"));
    client
        .execute("default", mk_db.build())
        .await
        .expect("create db");
}

/// Batch #1 — the main scenario from the brief: 5 ops with mixed `after` +
/// `$query` dependencies, executed through a real TCP round-trip.
///
///   create_table (mk_table)
///     --after(Explicit)--> insert (ins)
///     --$query(DataFlow)--> read (rd1)
///     --after+$query(Both)--> update (upd)
///     --$query(DataFlow)--> read2 (rd2)
///
/// Verifies `edge_provenance` carries the expected `EdgeKind` per edge, AND
/// that the pure-`after` edge (mk_table -> ins) does NOT leak into a $query
/// reference — i.e. Phase A's "after is ordering-only" guarantee holds over
/// the real wire, not just in a unit test.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn batch_mixed_after_and_query_edges_report_expected_provenance() {
    let (handle, client, _password) = boot().await;
    make_db(&client).await;

    let mut mk_table = Batch::new();
    mk_table.id("mk-table");
    mk_table.create_repo("mr", ddl::create_repo("main"));
    mk_table.create_table("tb", ddl::create_table("items").repo("main"));
    client
        .execute("seqdb", mk_table.build())
        .await
        .expect("create repo+table");

    let mut b = Batch::new();
    b.id("chain");

    // 1. insert — pure `after` on nothing (first op in this batch); we
    //    instead chain it after a no-op silent marker query to guarantee at
    //    least one pure-`after` (Explicit) edge in THIS batch, since
    //    create_table already ran in a prior batch.
    let marker = b.query_silent("marker", Query::from("items").limit(0));
    let ins = b.insert_after(
        "ins",
        insert("items").rows([doc! { "sku" => "A1", "qty" => 1_i64 }]),
        &[&marker],
    );

    // 2. read — pure `$query` data-flow dependency on `ins` (no `after`).
    let rd1 = b.query(
        "rd1",
        Query::from("items").where_eq("sku", ins.first().field("sku")),
    );

    // 3. update — BOTH an explicit `after` on `ins` AND a `$query` ref on
    //    `rd1` => EdgeKind::Both for the rd1 edge, EdgeKind::Explicit for
    //    the `ins` edge (ins has no $query ref from `upd`).
    let upd = b.update_after(
        "upd",
        update("items")
            .where_(shamir_client::builder::filter::eq(
                "sku",
                rd1.first().field("sku"),
            ))
            .set(doc! { "qty" => 2_i64 })
            .returning(shamir_client::builder::write::UpdateReturnMode::All),
        &[&ins],
    );
    b.after(&upd, &rd1); // add the $query-independent explicit edge too

    // 4. read2 — pure `$query` data-flow dependency on `upd`.
    let _rd2 = b.query(
        "rd2",
        Query::from("items").where_eq("sku", upd.first().field("sku")),
    );

    let resp = client
        .execute("seqdb", b.build())
        .await
        .expect("mixed chain batch");

    // ---- data correctness ----
    let rd1_res = resp.results.get("rd1").expect("rd1 alias");
    assert_eq!(rd1_res.records.len(), 1);
    assert_eq!(rd1_res.records[0].get_value_i64("qty"), Some(1));

    let rd2_res = resp.results.get("rd2").expect("rd2 alias");
    assert_eq!(rd2_res.records.len(), 1);
    assert_eq!(rd2_res.records[0].get_value_i64("qty"), Some(2));

    // ---- edge_provenance correctness (the actual point of this test) ----
    let prov = &resp.edge_provenance;

    // ins -> marker: pure `after`, no data flow => Explicit.
    let ins_prov = prov.get("ins").expect("ins must have provenance entry");
    assert_eq!(
        ins_prov.get("marker"),
        Some(&EdgeKind::Explicit),
        "ins->marker must be Explicit (pure `after`, no $query): {ins_prov:?}"
    );

    // rd1 -> ins: pure `$query`, no `after` => DataFlow.
    let rd1_prov = prov.get("rd1").expect("rd1 must have provenance entry");
    assert_eq!(
        rd1_prov.get("ins"),
        Some(&EdgeKind::DataFlow),
        "rd1->ins must be DataFlow (pure $query, no after): {rd1_prov:?}"
    );

    // upd -> rd1: both `after` (added explicitly) AND `$query` (via
    // rd1.first().field("sku")) => Both.
    let upd_prov = prov.get("upd").expect("upd must have provenance entry");
    assert_eq!(
        upd_prov.get("rd1"),
        Some(&EdgeKind::Both),
        "upd->rd1 must be Both (after + $query on same alias): {upd_prov:?}"
    );

    // upd -> ins: pure `after` (insert_after), no $query ref from upd to ins
    // directly => Explicit.
    assert_eq!(
        upd_prov.get("ins"),
        Some(&EdgeKind::Explicit),
        "upd->ins must be Explicit (pure `after`, no direct $query): {upd_prov:?}"
    );

    // rd2 -> upd: pure `$query` => DataFlow.
    let rd2_prov = prov.get("rd2").expect("rd2 must have provenance entry");
    assert_eq!(
        rd2_prov.get("upd"),
        Some(&EdgeKind::DataFlow),
        "rd2->upd must be DataFlow (pure $query, no after): {rd2_prov:?}"
    );

    client.close().await;
    handle.shutdown().await;
}

/// Regression e2e for Phase A, point 3: a PURE `after` dependency (no
/// `$query`) must NOT give the dependent op access to the predecessor's
/// data. Proven here through the REAL server/wire, not a planner unit test.
///
/// Scenario: `ins` inserts a row. `rd` runs `after(&rd, &ins)` only (no
/// `$query` reference into `ins`'s result) and reads the SAME table with an
/// unconditional filter. `rd` must see the row because it independently
/// queries the table (real data on disk) — but critically, if we instead ask
/// `rd` to filter using a bogus one-shot `$query`-shaped hand-rolled path
/// that does NOT correspond to a query-planner-registered reference, the
/// planner must not silently resolve it via the `after` edge. We assert
/// this indirectly: an `after`-only edge is reported as `Explicit` (not
/// `DataFlow`/`Both`) in `edge_provenance`, proving the wire-level plan never
/// treated the ordering hint as a data channel.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pure_after_dependency_does_not_open_data_flow_over_real_wire() {
    let (handle, client, _password) = boot().await;
    make_db(&client).await;

    let mut mk_table = Batch::new();
    mk_table.id("mk-table");
    mk_table.create_repo("mr", ddl::create_repo("main"));
    mk_table.create_table("tb", ddl::create_table("items").repo("main"));
    client
        .execute("seqdb", mk_table.build())
        .await
        .expect("create repo+table");

    let mut b = Batch::new();
    b.id("after-only");
    let ins = b.insert(
        "ins",
        insert("items").rows([doc! { "sku" => "B1", "qty" => 7_i64 }]),
    );
    // `rd` is ordered after `ins` but has NO `$query` reference to `ins` at
    // all — it just re-reads the table unconditionally.
    let _rd = b.query_after("rd", Query::from("items"), &[&ins]);

    let resp = client
        .execute("seqdb", b.build())
        .await
        .expect("after-only batch");

    let rd_res = resp.results.get("rd").expect("rd alias");
    // rd sees the row because it independently re-read the table after
    // `ins` committed (real data on disk) — NOT because `after` opened any
    // data channel. This is the observable behavior a fully-in-process unit
    // test can't distinguish from a bug where `after` silently resolves
    // like `$query` — but the edge_provenance assertion below can.
    assert_eq!(rd_res.records.len(), 1);
    assert_eq!(rd_res.records[0].get_value_str("sku"), Some("B1"));

    let prov = resp
        .edge_provenance
        .get("rd")
        .expect("rd must have a provenance entry for its after-dependency");
    assert_eq!(
        prov.get("ins"),
        Some(&EdgeKind::Explicit),
        "rd->ins must be Explicit-only: an `after`-only edge must never be \
         reported as DataFlow/Both, proving `after` carries no data \
         reference over the real wire: {prov:?}"
    );

    client.close().await;
    handle.shutdown().await;
}

/// A batch whose `after` + `$query` edges together form a cycle must fail
/// with a wire-level `CircularDependency` error (surfaced as
/// `ClientError::Db { code: "validation", .. }`) — not a 500 / panic /
/// connection drop. Proven through the real client/server round-trip.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn batch_with_after_query_cycle_returns_circular_dependency_over_wire() {
    let (handle, client, _password) = boot().await;
    make_db(&client).await;

    let mut mk_table = Batch::new();
    mk_table.id("mk-table");
    mk_table.create_repo("mr", ddl::create_repo("main"));
    mk_table.create_table("tb", ddl::create_table("items").repo("main"));
    client
        .execute("seqdb", mk_table.build())
        .await
        .expect("create repo+table");

    // Build a genuine cycle: `b2` depends on `a2` via `$query`
    // (EdgeKind::DataFlow), and `a2` is made `after` `b2` (EdgeKind::Explicit)
    // — a mixed after+$query cycle. `Batch::try_build` only catches
    // self-references/unknown aliases client-side; a cycle across two
    // distinct aliases is caught by the server-side planner, so we go
    // through the ordinary `build()` (no client-side cycle check) to prove
    // the SERVER rejects it over the wire.
    let mut b2 = Batch::new();
    b2.id("cycle2");
    let a2 = b2.query("a2", Query::from("items"));
    let b2h = b2.query(
        "b2",
        Query::from("items").where_eq("sku", a2.first().field("sku")),
    );
    b2.after(&a2, &b2h);

    let resp = client.execute("seqdb", b2.build()).await;
    match resp {
        Err(ClientError::Db { code, message }) => {
            assert_eq!(code, "validation", "unexpected error code: {message}");
            assert!(
                message.contains("Circular") || message.contains("circular"),
                "expected a circular-dependency message, got: {message}"
            );
        }
        other => panic!(
            "expected ClientError::Db{{code: \"validation\", ..}} for a circular batch, got: {other:?}"
        ),
    }

    client.close().await;
    handle.shutdown().await;
}
