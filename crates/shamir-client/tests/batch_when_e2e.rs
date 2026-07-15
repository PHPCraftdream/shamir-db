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
//! # KNOWN ENGINE BUG — found while writing this e2e test, NOT fixed here
//!
//! Per this task's brief, production code from Phases A-D is out of scope —
//! bugs are reported, not silently patched. This file documents a real
//! functional gap rather than exercising the (currently unreachable)
//! canonical "read balance -> conditionally debit or decline" scenario the
//! brief and the ADR describe:
//!
//! `QueryRunner::resolve_skip`
//! (`crates/shamir-engine/src/query/batch/query_runner.rs:120-140`)
//! evaluates a `when` filter against an EMPTY SYNTHETIC RECORD, using a
//! FRESH scratch `Interner::new()` (never the real table interner):
//!
//! ```ignore
//! let scratch = shamir_types::core::interner::Interner::new();
//! let ctx = FilterContext::new(&scratch, resolved_refs)...;
//! let node = crate::query::filter::compile_filter(filter, &scratch);
//! !node.matches(&InnerValue::Null, &ctx)
//! ```
//!
//! `compile_filter` (`crates/shamir-engine/src/query/filter/compile.rs`)
//! resolves EVERY comparison operator's `field` path via
//! `intern_field_path_compact(field, interner)` — a LOOKUP
//! (`interner.get_ind(part)`), never an insert. On the scratch interner
//! (freshly created, zero registered field names) this lookup returns
//! `None` for ANY field name, so `compile_compare` (used by
//! `Eq`/`Ne`/`Gt`/`Gte`/`Lt`/`Lte`/`FieldEq`) ALWAYS folds to
//! `FilterNode::False` — regardless of the RHS value, even when the RHS is
//! a `$query` ref carrying real cross-query data. `IsNull` always folds to
//! `FilterNode::True`, `IsNotNull` always folds to `FilterNode::False`, for
//! the same reason (`compile.rs:38-45`).
//!
//! Net effect: there is NO way today to express a data-driven `when`
//! condition of the ADR's own intended shape — "run this op iff
//! `$query_ref_A >= $query_ref_B`" (e.g. "iff balance >= amount") —
//! through any field-based `Filter` variant. The ADR's design note
//! (`docs/dev-artifacts/design/oql-03-conditional-execution-adr.md:44-50`,
//! "no `FieldRef` support needed/meaningful — only
//! `$query`/`$fn`/`$param`/literals matter for `when`") states the INTENDED
//! semantics (pure value-vs-value comparison, ignoring the record), but no
//! `Filter` variant implements a field-free value-vs-value comparison — every
//! comparison variant is hard-wired to a real field path that can never
//! resolve against the synthetic record's fresh interner. Only the
//! constant-fold outcomes (`IsNull` -> always true, `IsNotNull` -> always
//! false, `Gte`/`Eq`/etc. -> always false) are reachable in practice.
//!
//! This was verified directly: a temporary probe test built exactly the
//! ADR's canonical shape —
//! `Filter::Gte { field: ["balance"], value: $query-ref-to-100 }` with
//! `amount = 40` (should evaluate `true`, i.e. balance 100 >= 40) — and
//! observed `skipped: true` (i.e. the filter evaluated `false`) regardless
//! of the actual data. The probe was reverted; it is not part of this
//! commit's diff. Track the real fix (implementing a field-free value
//! comparison, or having `resolve_skip` special-case comparisons whose
//! field cannot intern by treating them as a pure `FilterValue` compare
//! ignoring the field) under a dedicated follow-up task, not Epic03/E.
//!
//! Every scenario below therefore uses `IsNull`/`IsNotNull`-based `when`
//! guards — the deterministic true/false primitives Phase B/C's own unit
//! tests (`when_skip_tests.rs`) rely on — which is the only mechanism able
//! to prove the executor's skip/cascade/wire semantics over a REAL wire
//! round-trip today. The "read balance -> debit or decline" narrative is
//! preserved by choosing WHICH deterministic guard fires (i.e. we drive the
//! branch decision from a Rust-side `bool` at test-authoring time, since the
//! engine itself cannot yet derive it from real query data), and the ledger
//! query in each scenario proves via real inserted rows that exactly the
//! expected branch actually ran.

use std::path::PathBuf;

use tempfile::TempDir;
use zeroize::Zeroizing;

use shamir_client::builder::batch::Batch;
use shamir_client::builder::ddl;
use shamir_client::builder::doc;
use shamir_client::builder::filter::{is_not_null, is_null};
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

/// Scenario 1: sufficient-balance branch — the "debit" insert runs (guard
/// `IsNull` on a field the empty synthetic record can never have -> always
/// `true`), the "decline" insert is skipped (complementary `IsNotNull` on
/// the same field -> always `false`). Both registered via `Batch::when`,
/// inside `transactional()`, after a `balance_check` read — the canonical
/// "read balance -> debit or decline" shape from the ADR/roadmap, modulo
/// the field-based-comparison gap documented in this file's header (the
/// guard direction is fixed at test-authoring time rather than derived from
/// `balance_check`'s data, since the engine cannot do that yet).
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
    let _ = &balance_check;

    let debit = b.insert(
        "debit",
        insert("ledger").rows([doc! { "owner" => "alice", "kind" => "debit", "amount" => 40_i64 }]),
    );
    // Guard that always evaluates true against the empty synthetic record
    // (see file header for why a real balance-vs-amount comparison isn't
    // reachable today): IsNull on a field that structurally can never be
    // present.
    b.when(&debit, is_null(vec!["never_present_field"]));

    let decline = b.insert(
        "decline",
        insert("ledger")
            .rows([doc! { "owner" => "alice", "kind" => "decline", "amount" => 40_i64 }]),
    );
    // Complementary guard: IsNotNull on the same never-present field always
    // evaluates false.
    b.when(&decline, is_not_null(vec!["never_present_field"]));

    let resp = client.execute(db, b.build()).await.expect("txn-sufficient");

    let debit_result = resp.results.get("debit").expect("debit alias");
    assert!(
        !debit_result.skipped,
        "debit must run (its when guard is IsNull(missing field) -> true): {debit_result:?}"
    );
    let decline_result = resp.results.get("decline").expect("decline alias");
    assert!(
        decline_result.skipped,
        "decline must be skipped (its when guard is IsNotNull(missing field) -> false): {decline_result:?}"
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
    let _ = &balance_check;

    let debit = b.insert(
        "debit",
        insert("ledger").rows([doc! { "owner" => "bob", "kind" => "debit", "amount" => 40_i64 }]),
    );
    // Swapped relative to Scenario 1: IsNotNull -> always false -> skipped.
    b.when(&debit, is_not_null(vec!["never_present_field"]));

    let decline = b.insert(
        "decline",
        insert("ledger").rows([doc! { "owner" => "bob", "kind" => "decline", "amount" => 40_i64 }]),
    );
    // IsNull -> always true -> runs.
    b.when(&decline, is_null(vec!["never_present_field"]));

    let resp = client
        .execute(db, b.build())
        .await
        .expect("txn-insufficient");

    let debit_result = resp.results.get("debit").expect("debit alias");
    assert!(
        debit_result.skipped,
        "debit must be skipped (guard IsNotNull(missing field) -> false): {debit_result:?}"
    );
    let decline_result = resp.results.get("decline").expect("decline alias");
    assert!(
        !decline_result.skipped,
        "decline must run (guard IsNull(missing field) -> true): {decline_result:?}"
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
/// Since case conditions built from real query data hit the same
/// field-based-comparison gap documented in this file's header, this
/// scenario drives the case selection through `IsNull`/`IsNotNull` guards
/// on a synthetic field, structured so exactly one of the three
/// `Batch::switch`-generated guards is true — proving the executor
/// actually evaluates and enforces `switch`'s AND/NOT/OR combinator chain
/// over the wire (a bug in that chaining would surface as either zero or
/// more than one branch running).
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
