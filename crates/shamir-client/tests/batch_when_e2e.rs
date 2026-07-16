//! End-to-end proof of OQL Epic 03 `when`/`switch` conditional execution
//! (Phases A-D) over a REAL wire round-trip: real `ServerLauncher` (TCP),
//! real `shamir_client::Client`, real SCRAM handshake — not an in-process
//! planner call.
//!
//! Mirrors the boot/connect pattern of `batch_cond_e2e.rs` (Epic02/D) and
//! `batch_sequencing_e2e.rs` (Epic01/D), but focuses on `Batch::when` /
//! `Batch::switch` — conditional op EXECUTION (an op either runs or is
//! `skipped: true`), not conditional VALUE evaluation.
//!
//! Task #648 — OQL Epic 03 / Phase E.
//!
//! # #651 FIXED — real data-driven `when` conditions now work
//!
//! Earlier versions of this file documented a critical engine bug (#651):
//! `QueryRunner::resolve_skip` evaluated `when` against an EMPTY SYNTHETIC
//! RECORD through a FRESH scratch `Interner::new()`, so every field-based
//! comparison variant (`Eq`/`Ne`/`Gt`/`Gte`/`Lt`/`Lte`/`FieldEq`) ALWAYS
//! folded to a fixed result regardless of the RHS `$query` data — the ADR's
//! own canonical scenario ("run this op iff `$query_ref_A >= $query_ref_B`",
//! e.g. "debit iff balance >= amount") was silently unreachable.
//!
//! The fix: `Filter::ValueCompare { left, cmp, right }` — a value-vs-value
//! comparison with NO field/record dependency. Both sides resolve via
//! `resolve_filter_query` (the same `$query`/`$fn`/`$param`/literal
//! resolution `$cond`/`$expr` already use) at MATCH time against the
//! current `resolved_refs`, so a real cross-query comparison is finally
//! reachable inside `when`. The old field-based variants are UNCHANGED for
//! real per-row WHERE-clause filtering, but using one inside `when` is now
//! REJECTED at plan time with `BatchError::InvalidWhenFilter` instead of
//! silently folding — see `when_field_based_comparison_is_rejected_at_plan_time`
//! in `crates/shamir-engine/src/query/batch/tests/executor_tests/when_skip_tests.rs`.
//!
//! Scenarios 1 and 2 below now drive the debit/decline branch selection
//! from REAL query data (`balance_check`'s fetched balance vs. a literal
//! `amount`) via `Filter::ValueCompare`, exactly the ADR's canonical shape.
//! Scenario 3 (`switch`) still uses `IsNull`/`IsNotNull` guards on a
//! synthetic field — that pattern remains a legitimate presence-guard
//! idiom (ADR Decision 1), not a workaround for this bug.

use std::path::PathBuf;

use tempfile::TempDir;
use zeroize::Zeroizing;

use shamir_client::builder::batch::Batch;
use shamir_client::builder::ddl;
use shamir_client::builder::doc;
use shamir_client::builder::filter::{is_not_null, is_null, value_gte, value_lt};
use shamir_client::builder::write::insert;
use shamir_client::builder::Query;
use shamir_client::{Client, ConnectOptions};

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

/// Creates db `whendb` with repo `main` / tables `accounts` (balance seed)
/// and `ledger` (debit/decline records), ready for DML.
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
    mk_table.create_table("tb-accounts", ddl::create_table("accounts").repo("main"));
    mk_table.create_table("tb-ledger", ddl::create_table("ledger").repo("main"));
    client
        .execute(db, mk_table.build())
        .await
        .expect("create repo+tables");
}

/// Scenario 1: sufficient-balance branch — the "debit" insert runs because
/// its `value_gte` guard (`balance_check`'s fetched balance >= literal
/// `amount`) genuinely evaluates `true` against the REAL seeded data
/// (100 >= 40); the "decline" insert's complementary `value_lt` guard
/// (`balance < amount`) evaluates `false` and is skipped. Both registered
/// via `Batch::when`, inside `transactional()`, after a `balance_check`
/// read — the ADR's own canonical "read balance -> debit or decline"
/// shape, now genuinely data-driven (#651 fix).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn when_sufficient_balance_runs_debit_and_skips_decline_over_real_wire() {
    let (handle, client, _password) = boot().await;
    let db = "whendb_sufficient";
    make_db(&client, db).await;

    // Seed one account with balance = 100.
    let mut seed = Batch::new();
    seed.id("seed");
    seed.insert(
        "acc",
        insert("accounts").rows([doc! { "owner" => "alice", "balance" => 100_i64 }]),
    );
    client.execute(db, seed.build()).await.expect("seed");

    // Transactional batch: read balance -> conditionally debit or decline.
    let mut b = Batch::new();
    b.id("txn-sufficient");
    b.transactional();
    let balance_check = b.query(
        "balance_check",
        Query::from("accounts").where_eq("owner", "alice"),
    );

    let debit = b.insert(
        "debit",
        insert("ledger").rows([doc! { "owner" => "alice", "kind" => "debit", "amount" => 40_i64 }]),
    );
    // Real data-driven guard (#651 fix): balance_check's fetched balance
    // (100) >= the literal amount (40) genuinely evaluates true.
    b.when(
        &debit,
        value_gte(balance_check.first().field("balance"), 40_i64),
    );

    let decline = b.insert(
        "decline",
        insert("ledger")
            .rows([doc! { "owner" => "alice", "kind" => "decline", "amount" => 40_i64 }]),
    );
    // Complementary guard: balance < amount genuinely evaluates false.
    b.when(
        &decline,
        value_lt(balance_check.first().field("balance"), 40_i64),
    );

    let resp = client.execute(db, b.build()).await.expect("txn-sufficient");

    let debit_result = resp.results.get("debit").expect("debit alias");
    assert!(
        !debit_result.skipped,
        "debit must run (balance 100 >= amount 40 via ValueCompare): {debit_result:?}"
    );
    let decline_result = resp.results.get("decline").expect("decline alias");
    assert!(
        decline_result.skipped,
        "decline must be skipped (balance 100 < amount 40 is false via ValueCompare): {decline_result:?}"
    );

    // Cross-check via a fresh read: the ledger must contain exactly the
    // debit row and no decline row.
    let mut q = Batch::new();
    q.id("verify");
    q.query(
        "ledger_rows",
        Query::from("ledger").where_eq("owner", "alice"),
    );
    let vresp = client.execute(db, q.build()).await.expect("verify read");
    let rows = &vresp
        .results
        .get("ledger_rows")
        .expect("ledger_rows")
        .records;
    assert_eq!(rows.len(), 1, "exactly one ledger row must exist: {rows:?}");
    assert_eq!(
        rows[0].get_value_str("kind"),
        Some("debit"),
        "the surviving ledger row must be the debit, not the decline: {rows:?}"
    );

    client.close().await;
    handle.shutdown().await;
}

/// Scenario 2: insufficient-balance branch — mirror of Scenario 1 with the
/// guards swapped, proving both directions round-trip correctly over the
/// real wire (not just "always true wins" by coincidence).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn when_insufficient_balance_skips_debit_and_runs_decline_over_real_wire() {
    let (handle, client, _password) = boot().await;
    let db = "whendb_insufficient";
    make_db(&client, db).await;

    // Seed one account with balance = 10 (insufficient for amount = 40).
    let mut seed = Batch::new();
    seed.id("seed");
    seed.insert(
        "acc",
        insert("accounts").rows([doc! { "owner" => "bob", "balance" => 10_i64 }]),
    );
    client.execute(db, seed.build()).await.expect("seed");

    let mut b = Batch::new();
    b.id("txn-insufficient");
    b.transactional();
    let balance_check = b.query(
        "balance_check",
        Query::from("accounts").where_eq("owner", "bob"),
    );

    let debit = b.insert(
        "debit",
        insert("ledger").rows([doc! { "owner" => "bob", "kind" => "debit", "amount" => 40_i64 }]),
    );
    // Real data-driven guard: balance (10) >= amount (40) is false.
    b.when(
        &debit,
        value_gte(balance_check.first().field("balance"), 40_i64),
    );

    let decline = b.insert(
        "decline",
        insert("ledger").rows([doc! { "owner" => "bob", "kind" => "decline", "amount" => 40_i64 }]),
    );
    // Complementary guard: balance (10) < amount (40) is true.
    b.when(
        &decline,
        value_lt(balance_check.first().field("balance"), 40_i64),
    );

    let resp = client
        .execute(db, b.build())
        .await
        .expect("txn-insufficient");

    let debit_result = resp.results.get("debit").expect("debit alias");
    assert!(
        debit_result.skipped,
        "debit must be skipped (balance 10 >= amount 40 is false via ValueCompare): {debit_result:?}"
    );
    let decline_result = resp.results.get("decline").expect("decline alias");
    assert!(
        !decline_result.skipped,
        "decline must run (balance 10 < amount 40 via ValueCompare): {decline_result:?}"
    );

    let mut q = Batch::new();
    q.id("verify");
    q.query(
        "ledger_rows",
        Query::from("ledger").where_eq("owner", "bob"),
    );
    let vresp = client.execute(db, q.build()).await.expect("verify read");
    let rows = &vresp
        .results
        .get("ledger_rows")
        .expect("ledger_rows")
        .records;
    assert_eq!(rows.len(), 1, "exactly one ledger row must exist: {rows:?}");
    assert_eq!(
        rows[0].get_value_str("kind"),
        Some("decline"),
        "the surviving ledger row must be the decline, not the debit: {rows:?}"
    );

    client.close().await;
    handle.shutdown().await;
}

/// Scenario 3: `Batch::switch` with 3 branches over a real wire round-trip
/// — mirrors the vip/regular/newbie shape from Epic02/D, but now as
/// conditional EXECUTION of three DIFFERENT ops (three distinct inserts
/// into `ledger`), not conditional VALUE selection. Exactly one branch's
/// insert must run; the server-computed complementary `when` guards
/// (`Batch::switch`'s AND(NOT prior) chaining) must make the other two
/// skip, over the real engine — not merely in the builder's local state.
///
/// This scenario drives the case selection through `IsNull`/`IsNotNull`
/// guards on a synthetic field (a legitimate presence-guard idiom, ADR
/// Decision 1 — not a workaround), structured so exactly one of the three
/// `Batch::switch`-generated guards is true — proving the executor
/// actually evaluates and enforces `switch`'s AND/NOT/OR combinator chain
/// over the wire (a bug in that chaining would surface as either zero or
/// more than one branch running). Scenarios 1/2 above already cover the
/// real data-driven `ValueCompare` path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn switch_three_branches_executes_exactly_one_over_real_wire() {
    let (handle, client, _password) = boot().await;
    let db = "whendb_switch3";
    make_db(&client, db).await;

    let mut b = Batch::new();
    b.id("txn-switch3");
    b.transactional();

    // Case 1's condition (IsNull on a missing field) is always `true`, so
    // `Batch::switch` must select case 1 and skip both case 2 and default
    // regardless of their own conditions (case2's guard becomes
    // AND(NOT case1, case2) = AND(false, ...) = false; default's guard
    // becomes NOT(OR(case1, case2)) = NOT(true) = false).
    let handles = b.switch(
        vec![
            (
                "vip_insert",
                is_null(vec!["never_present_field"]),
                insert("ledger").rows([doc! { "owner" => "carol", "kind" => "vip" }]),
            ),
            (
                "regular_insert",
                is_not_null(vec!["also_never_present"]),
                insert("ledger").rows([doc! { "owner" => "carol", "kind" => "regular" }]),
            ),
        ],
        (
            "newbie_insert",
            insert("ledger").rows([doc! { "owner" => "carol", "kind" => "newbie" }]),
        ),
    );
    assert_eq!(handles.len(), 3, "switch must register 3 entries");

    let resp = client.execute(db, b.build()).await.expect("txn-switch3");

    let vip = resp.results.get("vip_insert").expect("vip_insert alias");
    let regular = resp
        .results
        .get("regular_insert")
        .expect("regular_insert alias");
    let newbie = resp
        .results
        .get("newbie_insert")
        .expect("newbie_insert alias");

    assert!(
        !vip.skipped,
        "vip_insert's condition (IsNull(missing) -> true) must run: {vip:?}"
    );
    assert!(
        regular.skipped,
        "regular_insert must be skipped: its complementary guard is \
         AND(NOT vip_condition, regular_condition) = AND(false, ...) = false: {regular:?}"
    );
    assert!(
        newbie.skipped,
        "newbie_insert (default) must be skipped: its guard is \
         NOT(OR(vip_condition, regular_condition)) = NOT(true) = false: {newbie:?}"
    );

    // Cross-check via a fresh read: the ledger must contain exactly the vip
    // row and neither of the other two.
    let mut q = Batch::new();
    q.id("verify");
    q.query(
        "ledger_rows",
        Query::from("ledger").where_eq("owner", "carol"),
    );
    let vresp = client.execute(db, q.build()).await.expect("verify read");
    let rows = &vresp
        .results
        .get("ledger_rows")
        .expect("ledger_rows")
        .records;
    assert_eq!(rows.len(), 1, "exactly one ledger row must exist: {rows:?}");
    assert_eq!(
        rows[0].get_value_str("kind"),
        Some("vip"),
        "the surviving ledger row must be from the vip branch: {rows:?}"
    );

    client.close().await;
    handle.shutdown().await;
}
