//! End-to-end proof of OQL Epic 04 `for_each` data-dependent loop
//! (Phases A-D) over a REAL wire round-trip: real `ServerLauncher` (TCP),
//! real `shamir_client::Client`, real SCRAM handshake — not an in-process
//! planner call.
//!
//! Mirrors the boot/connect pattern of `batch_when_e2e.rs` (Epic03/E), but
//! for `Batch::for_each`.
//!
//! Task #656 — OQL Epic 04 / Phase E.
//!
//! Unlike Epic03's `when` (which hit a real blocking bug — field-based
//! comparisons always fold to a fixed result, #651, still open — forcing
//! that e2e file to use synthetic `IsNull`/`IsNotNull` guards instead of
//! real data-driven conditions), `ForEach`'s `over` has NO such limitation:
//! `over` can be a genuine `$query` column reference, resolved once
//! against real data, with no scratch-interner involved (see
//! `docs/dev-artifacts/design/oql-04-loops-foreach-adr.md`'s "Bug #651 —
//! independence of `bind_row`" section). So this file exercises the REAL,
//! INTENDED, canonical scenario directly — no workaround needed.

use std::path::PathBuf;

use tempfile::TempDir;
use zeroize::Zeroizing;

use shamir_client::builder::batch::Batch;
use shamir_client::builder::ddl;
use shamir_client::builder::val::{lit, param};
use shamir_client::builder::write::{doc, insert};
use shamir_client::builder::Query;
use shamir_client::{BatchRequest, Client, ConnectOptions};

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

/// Creates db `db` with repo `main` / tables `orders` (seed with order rows)
/// and `audit_log` (empty), ready for DML.
async fn make_db(client: &Client, db: &str) {
    let mut mk_db = Batch::new();
    mk_db.id("mk-db");
    mk_db.create_db("mk", ddl::create_db(db));
    client
        .execute("default", mk_db.build())
        .await
        .expect("create db");

    let mut mk_table = Batch::new();
    mk_table.id("mk-table");
    mk_table.create_repo("mr", ddl::create_repo("main"));
    mk_table.create_table("tb-orders", ddl::create_table("orders").repo("main"));
    mk_table.create_table("tb-audit", ddl::create_table("audit_log").repo("main"));
    client
        .execute(db, mk_table.build())
        .await
        .expect("create repo+tables");
}

/// Builds the inner loop body: insert one row into `audit_log`, with
/// `order_id` bound to the current loop element (`$param <bind_row>`) and a
/// fixed `note`.
fn audit_insert_body(bind_row: &str) -> BatchRequest {
    let mut inner = Batch::new();
    inner.insert(
        "audit",
        insert("audit_log").rows([doc()
            .set("order_id", param(bind_row))
            .set("note", "audited")
            .build()]),
    );
    inner.build()
}

/// Scenario 1 (canonical): read all order ids for a customer via a real
/// `$query` column ref, then for_each-insert one `audit_log` row per order,
/// each row referencing that order's REAL id — proving real cross-query
/// data flows through `over` and `bind_row` over the wire.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn for_each_over_query_column_ref_inserts_one_audit_row_per_order_over_real_wire() {
    let (handle, client, _password) = boot().await;
    let db = "fedb_basic";
    make_db(&client, db).await;

    // Seed 3 orders for "alice" (with explicit, distinct `order_id`
    // values — the field the loop's `over` column-ref reads), 1 for "bob"
    // (to prove the where_eq filter narrows `over` to only alice's
    // orders).
    let expected_ids: Vec<i64> = vec![1001, 1002, 1003];
    let mut seed = Batch::new();
    seed.id("seed");
    seed.insert(
        "o1",
        insert("orders").rows([doc()
            .set("order_id", expected_ids[0])
            .set("customer_id", "alice")
            .set("amount", 10_i64)
            .build()]),
    );
    seed.insert(
        "o2",
        insert("orders").rows([doc()
            .set("order_id", expected_ids[1])
            .set("customer_id", "alice")
            .set("amount", 20_i64)
            .build()]),
    );
    seed.insert(
        "o3",
        insert("orders").rows([doc()
            .set("order_id", expected_ids[2])
            .set("customer_id", "alice")
            .set("amount", 30_i64)
            .build()]),
    );
    seed.insert(
        "o4",
        insert("orders").rows([doc()
            .set("order_id", 2001_i64)
            .set("customer_id", "bob")
            .set("amount", 99_i64)
            .build()]),
    );
    client.execute(db, seed.build()).await.expect("seed");

    // Transactional batch: read alice's orders -> for_each-insert one
    // audit_log row per order id.
    let mut b = Batch::new();
    b.id("txn-basic");
    b.transactional();
    let orders_q = b.query(
        "orders_q",
        Query::from("orders").where_eq("customer_id", "alice"),
    );
    let over_ref = orders_q.column("order_id");
    let inner = audit_insert_body("order_id");
    b.for_each("loop", over_ref, "order_id", inner);

    let resp = client.execute(db, b.build()).await.expect("txn-basic");

    let loop_result = resp.results.get("loop").expect("loop alias");
    assert!(!loop_result.skipped, "loop must run: {loop_result:?}");
    let list = loop_result
        .value
        .as_ref()
        .expect("loop must carry a value")
        .as_array()
        .expect("loop value must be a List");
    assert_eq!(
        list.len(),
        3,
        "loop result must have exactly 3 elements, one per seeded order: {list:?}"
    );

    // Cross-check: audit_log must have exactly 3 rows, one per real order id.
    let mut q = Batch::new();
    q.id("verify");
    q.query("audit_rows", Query::from("audit_log"));
    let vresp = client.execute(db, q.build()).await.expect("verify read");
    let rows = &vresp.results.get("audit_rows").expect("audit_rows").records;
    assert_eq!(
        rows.len(),
        3,
        "exactly one audit row per seeded order must exist: {rows:?}"
    );
    let mut actual_ids: Vec<i64> = rows
        .iter()
        .map(|r| {
            r.get_value_i64("order_id")
                .expect("audit row must have order_id")
        })
        .collect();
    actual_ids.sort_unstable();
    assert_eq!(
        actual_ids, expected_ids,
        "each audit row's order_id must match a REAL seeded order id \
         (proves `over` resolved real cross-query data end-to-end over the wire)"
    );
    for row in rows.iter() {
        assert_eq!(
            row.get_value_str("note"),
            Some("audited"),
            "every audit row must carry the loop body's fixed note field: {rows:?}"
        );
    }

    client.close().await;
    handle.shutdown().await;
}

/// Scenario 2: zero iterations — seed zero matching orders, assert the loop
/// result is an empty list and NO audit rows are inserted, over the real
/// wire.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn for_each_zero_matching_orders_produces_empty_list_and_no_audit_rows_over_real_wire() {
    let (handle, client, _password) = boot().await;
    let db = "fedb_zero";
    make_db(&client, db).await;

    // Seed an order for a DIFFERENT customer — "carol"'s query below will
    // match zero rows.
    let mut seed = Batch::new();
    seed.id("seed");
    seed.insert(
        "o1",
        insert("orders").rows([doc()
            .set("customer_id", "dave")
            .set("amount", 5_i64)
            .build()]),
    );
    client.execute(db, seed.build()).await.expect("seed");

    let mut b = Batch::new();
    b.id("txn-zero");
    b.transactional();
    let orders_q = b.query(
        "orders_q",
        Query::from("orders").where_eq("customer_id", "carol"),
    );
    let over_ref = orders_q.column("id");
    let inner = audit_insert_body("order_id");
    b.for_each("loop", over_ref, "order_id", inner);

    let resp = client.execute(db, b.build()).await.expect("txn-zero");

    let loop_result = resp.results.get("loop").expect("loop alias");
    assert!(!loop_result.skipped, "loop must run: {loop_result:?}");
    let list = loop_result
        .value
        .as_ref()
        .expect("loop must carry a value")
        .as_array()
        .expect("loop value must be a List");
    assert_eq!(
        list.len(),
        0,
        "zero matching orders must produce zero iterations: {list:?}"
    );

    let mut q = Batch::new();
    q.id("verify");
    q.query("audit_rows", Query::from("audit_log"));
    let vresp = client.execute(db, q.build()).await.expect("verify read");
    let rows = &vresp.results.get("audit_rows").expect("audit_rows").records;
    assert_eq!(
        rows.len(),
        0,
        "no audit rows must exist when zero orders matched: {rows:?}"
    );

    client.close().await;
    handle.shutdown().await;
}

/// Scenario 3: literal-array `over` — a `for_each` whose `over` is a small
/// literal array of ids (not a `$query` ref), proving that source works
/// over the real wire too (not just in-process).
///
/// Also the #660 regression proof: the batch is transactional and its ONLY
/// top-level entry is a bare `ForEach` (no other `Read`/`Insert` alongside).
/// Before the fix `distinct_repos()` didn't walk into `Batch`/`ForEach`
/// bodies, so this shape failed with `"transactional batch has no data ops
/// to target a repo"` and this test carried a workaround (an extra
/// `orders_probe` top-level `Read` solely to supply a `table_ref()`). #660
/// is fixed — `distinct_repos()` now recurses into nested bodies — and the
/// workaround is removed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn for_each_over_literal_array_inserts_one_audit_row_per_literal_over_real_wire() {
    let (handle, client, _password) = boot().await;
    let db = "fedb_literal";
    make_db(&client, db).await;

    let mut b = Batch::new();
    b.id("txn-literal");
    b.transactional();
    let over_literal = vec![lit(101_i64), lit(202_i64), lit(303_i64)];
    let inner = audit_insert_body("order_id");
    b.for_each("loop", over_literal, "order_id", inner);

    let resp = client.execute(db, b.build()).await.expect("txn-literal");

    let loop_result = resp.results.get("loop").expect("loop alias");
    assert!(!loop_result.skipped, "loop must run: {loop_result:?}");
    let list = loop_result
        .value
        .as_ref()
        .expect("loop must carry a value")
        .as_array()
        .expect("loop value must be a List");
    assert_eq!(
        list.len(),
        3,
        "literal-array over with 3 elements must produce 3 iterations: {list:?}"
    );

    let mut q = Batch::new();
    q.id("verify");
    q.query("audit_rows", Query::from("audit_log"));
    let vresp = client.execute(db, q.build()).await.expect("verify read");
    let rows = &vresp.results.get("audit_rows").expect("audit_rows").records;
    assert_eq!(rows.len(), 3, "exactly 3 audit rows must exist: {rows:?}");
    let mut actual_ids: Vec<i64> = rows
        .iter()
        .map(|r| {
            r.get_value_i64("order_id")
                .expect("audit row must have order_id")
        })
        .collect();
    actual_ids.sort_unstable();
    assert_eq!(
        actual_ids,
        vec![101, 202, 303],
        "each audit row's order_id must match a literal from `over`: {rows:?}"
    );

    client.close().await;
    handle.shutdown().await;
}

/// Scenario 4: error mid-loop in a transactional batch — a unique index on
/// `audit_log.order_id` makes the second iteration (a duplicate order id)
/// fail, and the whole transactional batch (including a "good" insert that
/// ran before the loop) must roll back — no partial audit rows survive.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn for_each_iteration_error_mid_loop_rolls_back_whole_tx_over_real_wire() {
    let (handle, client, _password) = boot().await;
    let db = "fedb_txabort";
    make_db(&client, db).await;

    // Unique index on audit_log.order_id, so a duplicate order_id insert
    // fails mid-loop.
    let mut mk_index = Batch::new();
    mk_index.id("mk-index");
    mk_index.create_index(
        "ix",
        ddl::create_index("audit_log_order_id_uq", "audit_log")
            .field("order_id")
            .unique()
            .repo("main"),
    );
    client
        .execute(db, mk_index.build())
        .await
        .expect("create unique index");

    let mut b = Batch::new();
    b.id("txn-abort");
    b.transactional();
    // A "good" write that must be rolled back too when the loop fails.
    b.insert(
        "good",
        insert("orders").rows([doc()
            .set("customer_id", "erin")
            .set("amount", 1_i64)
            .build()]),
    );
    // over: [1, 1, 2] -- iteration 0 inserts order_id=1 (ok), iteration 1
    // duplicates order_id=1 (unique-index violation), iteration 2 must
    // never run.
    let over_literal = vec![lit(1_i64), lit(1_i64), lit(2_i64)];
    let inner = audit_insert_body("order_id");
    b.for_each("loop", over_literal, "order_id", inner);

    let resp = client.execute(db, b.build()).await;
    // Depending on how the client surfaces a mid-batch failure inside a
    // transactional batch, either `execute` itself errors, or it succeeds
    // with a TransactionInfo reporting "aborted". Handle both shapes.
    match resp {
        Ok(resp) => {
            let tx_info = resp
                .transaction
                .as_ref()
                .expect("transactional batch must carry TransactionInfo");
            assert_eq!(
                tx_info.status, "aborted",
                "a for_each iteration failure must abort the whole tx batch: {tx_info:?}"
            );
        }
        Err(e) => {
            // The transport-level error itself is proof the batch did not
            // commit; the row-count assertions below confirm no partial
            // writes survived.
            let _ = e;
        }
    }

    // Neither the "good" order insert nor any audit_log row must survive.
    let mut q = Batch::new();
    q.id("verify");
    q.query("orders_rows", Query::from("orders"));
    q.query("audit_rows", Query::from("audit_log"));
    let vresp = client.execute(db, q.build()).await.expect("verify read");
    let order_rows = &vresp
        .results
        .get("orders_rows")
        .expect("orders_rows")
        .records;
    let audit_rows = &vresp.results.get("audit_rows").expect("audit_rows").records;
    assert_eq!(
        order_rows.len(),
        0,
        "the 'good' order insert must have been rolled back too: {order_rows:?}"
    );
    assert_eq!(
        audit_rows.len(),
        0,
        "no partial audit rows must survive a mid-loop tx abort: {audit_rows:?}"
    );

    client.close().await;
    handle.shutdown().await;
}
