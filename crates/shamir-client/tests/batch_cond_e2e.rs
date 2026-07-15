//! End-to-end proof of OQL Epic 02 `$cond`/`switch_case` value evaluation
//! (Phases A/B/C) over a REAL wire round-trip: real `ServerLauncher` (TCP),
//! real `shamir_client::Client`, real SCRAM handshake — not an in-process
//! planner call.
//!
//! Mirrors the boot/connect pattern of `batch_sequencing_e2e.rs` (Epic01/D)
//! but focuses on `$cond`/`switch_case` used as a WHERE-filter comparison
//! value, evaluated by the real engine (`resolve_filter_query`).
//!
//! Per the correction discovered in Epic02/B (task #641): `$cond` does NOT
//! compose into write SET-values today (`UpdateOp.set`/`SetOp.value` are
//! typed as `QueryValue`, which structurally cannot hold
//! `FilterValue::Cond`). That gap is out of scope here — every scenario
//! below uses `$cond`/`switch_case` in a WHERE-filter comparison value,
//! which is the fully-supported path today.
//!
//! Task #638 — OQL Epic 02 / Phase D.

use std::path::PathBuf;

use tempfile::TempDir;
use zeroize::Zeroizing;

use shamir_client::builder::batch::Batch;
use shamir_client::builder::ddl;
use shamir_client::builder::doc;
use shamir_client::builder::filter::gte;
use shamir_client::builder::val::{cond, switch_case};
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

/// Creates db `conddb` with repo `main` / table `users`, ready for DML.
async fn make_db(client: &Client) {
    let mut mk_db = Batch::new();
    mk_db.id("mk-db");
    mk_db.create_db("mk", ddl::create_db("conddb"));
    client
        .execute("default", mk_db.build())
        .await
        .expect("create db");

    let mut mk_table = Batch::new();
    mk_table.id("mk-table");
    mk_table.create_repo("mr", ddl::create_repo("main"));
    mk_table.create_table("tb", ddl::create_table("users").repo("main"));
    client
        .execute("conddb", mk_table.build())
        .await
        .expect("create repo+table");
}

/// The canonical vip/regular/newbie `switch_case` classification, mirrored
/// from the `switch_case()` docstring example: `score >= 100 -> "vip"`,
/// `score >= 50 -> "regular"`, else `"newbie"`.
fn tier_switch_case() -> shamir_query_types::filter::FilterValue {
    switch_case(
        vec![
            (gte("score", 100_i64), "vip".into()),
            (gte("score", 50_i64), "regular".into()),
        ],
        "newbie",
    )
}

/// Scenario 1: insert several `users` with a `score` field, then run a
/// `read` whose WHERE-filter compares the record's `tier` field against a
/// value computed by `switch_case` (vip/regular/newbie classification by
/// `score`). The real server/engine must evaluate the `$cond` chain and
/// return only the correctly-classified records.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn switch_case_where_filter_classifies_records_over_real_wire() {
    let (handle, client, _password) = boot().await;
    make_db(&client).await;

    let mut b = Batch::new();
    b.id("seed");
    b.insert(
        "ins",
        insert("users").rows([
            doc! { "name" => "alice", "score" => 120_i64, "tier" => "vip" },
            doc! { "name" => "bob", "score" => 75_i64, "tier" => "regular" },
            doc! { "name" => "carol", "score" => 10_i64, "tier" => "newbie" },
            doc! { "name" => "dave", "score" => 120_i64, "tier" => "regular" },
        ]),
    );
    client.execute("conddb", b.build()).await.expect("seed");

    // WHERE tier == switch_case(score >= 100 -> vip, score >= 50 -> regular,
    // else newbie): the record's OWN `tier` field must match the tier
    // computed from its OWN `score` field to pass the filter.
    let mut q = Batch::new();
    q.id("rd");
    q.query(
        "rd",
        Query::from("users").where_eq("tier", tier_switch_case()),
    );
    let resp = client.execute("conddb", q.build()).await.expect("read");

    let rd = resp.results.get("rd").expect("rd alias");
    let mut names: Vec<&str> = rd
        .records
        .iter()
        .filter_map(|r| r.get_value_str("name"))
        .collect();
    names.sort_unstable();

    // alice (120, tier=vip) matches: switch_case(120) == "vip" == tier. OK.
    // bob (75, tier=regular) matches: switch_case(75) == "regular". OK.
    // carol (10, tier=newbie) matches: switch_case(10) == "newbie". OK.
    // dave (120, tier=regular) does NOT match: switch_case(120) == "vip" !=
    // "regular" — dave is deliberately mis-tagged to prove the engine
    // actually evaluates `$cond` per-record rather than short-circuiting to
    // a constant/default.
    assert_eq!(
        names,
        vec!["alice", "bob", "carol"],
        "engine must evaluate switch_case per-record and exclude dave \
         (score=120 classifies as vip, but his stored tier is regular): {rd:?}"
    );

    client.close().await;
    handle.shutdown().await;
}

/// Scenario 2: a nested `$cond` (3 levels deep, same switch-case pattern
/// hand-nested via `cond()` rather than the `switch_case()` sugar) evaluated
/// through the real wire. Proves the engine recurses through nested `$cond`
/// correctly rather than stopping at the first level / returning a
/// default/stub.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nested_three_level_cond_evaluates_correctly_over_real_wire() {
    let (handle, client, _password) = boot().await;
    make_db(&client).await;

    let mut b = Batch::new();
    b.id("seed");
    b.insert(
        "ins",
        insert("users").rows([
            doc! { "name" => "eve", "score" => 200_i64, "tier" => "vip" },
            doc! { "name" => "frank", "score" => 60_i64, "tier" => "regular" },
            doc! { "name" => "grace", "score" => 5_i64, "tier" => "newbie" },
        ]),
    );
    client.execute("conddb", b.build()).await.expect("seed");

    // Hand-nested 3-level $cond, structurally identical to switch_case's
    // output but built via cond(cond(cond(...))) directly, to prove the
    // engine's recursive evaluator handles arbitrary nesting depth, not
    // just the sugar-generated shape.
    let nested = cond(
        gte("score", 100_i64),
        "vip",
        cond(
            gte("score", 50_i64),
            "regular",
            cond(gte("score", 1_i64), "newbie", "unranked"),
        ),
    );

    let mut q = Batch::new();
    q.id("rd");
    q.query("rd", Query::from("users").where_eq("tier", nested));
    let resp = client.execute("conddb", q.build()).await.expect("read");

    let rd = resp.results.get("rd").expect("rd alias");
    let mut names: Vec<&str> = rd
        .records
        .iter()
        .filter_map(|r| r.get_value_str("name"))
        .collect();
    names.sort_unstable();

    assert_eq!(
        names,
        vec!["eve", "frank", "grace"],
        "engine must recurse through all 3 levels of nested $cond, not just \
         evaluate the outermost branch: {rd:?}"
    );

    client.close().await;
    handle.shutdown().await;
}

/// Scenario 3: a `$cond` whose branch is a `$query` reference into the
/// result of a PRIOR query in the same batch — a cross-query conditional
/// value.
///
/// # KNOWN ENGINE BUG — found while writing this e2e test, NOT fixed here
///
/// Per this task's brief, production code from Phases A/B/C is out of
/// scope — bugs are reported, not silently patched. This test documents a
/// real gap rather than asserting the (currently unreachable) intended
/// behavior:
///
/// `BatchPlanner::extract_deps_from_filter_value`
/// (`crates/shamir-query-types/src/batch/planner.rs:342-358`) only
/// recurses into `FilterValue::Array` and matches `FilterValue::QueryRef`
/// directly:
///
/// ```ignore
/// fn extract_deps_from_filter_value(value: &FilterValue, deps: &mut TSet<String>) {
///     match value {
///         FilterValue::Array(arr) => { for v in arr { Self::extract_deps_from_filter_value(v, deps); } }
///         FilterValue::QueryRef { alias, .. } => { deps.insert(Self::extract_base_alias(alias)); }
///         _ => {}   // <-- Cond, Expr, FnCall, FieldRef, Param all silently skipped
///     }
/// }
/// ```
///
/// So a `$query` ref nested inside a `$cond` branch used as a WHERE-filter
/// comparison value (exactly the `cross_cond` shape below) produces ZERO
/// extracted dependencies — the batch planner never adds a `DataFlow`/`Both`
/// edge for `rd -> threshold_lookup`. Even adding an EXPLICIT `after` edge
/// does not help: `query_runner::build_resolved_refs`
/// (`crates/shamir-engine/src/query/batch/query_runner.rs:31-48`) only
/// copies a dependency's result into the dependent op's `FilterContext` for
/// `EdgeKind::DataFlow`/`Both` edges (`kind.is_data_flow()`) — a pure
/// `EdgeKind::Explicit` `after` edge is ordering-only by design (the
/// Epic01/A guarantee) and must NOT leak data. Since the planner
/// mis-classifies this edge as `Explicit` (having failed to detect the
/// `$query` ref inside `$cond`), `threshold_lookup`'s result is legitimately
/// absent from `rd`'s `resolved_refs` even with an explicit `after` — the
/// `$cond`'s `$query`-ref branch resolves to `None` and the whole
/// comparison silently misses (see `resolve_filter_query`'s documented
/// silent-miss semantics, `crates/shamir-engine/src/query/filter/resolve.rs`).
///
/// This test therefore asserts the CURRENT (buggy) behavior — `heidi` is
/// silently excluded even though she should match — so it fails loudly the
/// moment `extract_deps_from_filter_value` gains a `Cond`/`Expr`/`FnCall`
/// arm and the underlying bug is fixed (a welcome regression to catch).
/// Track the real fix under a dedicated follow-up task, not Epic02/D.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cond_branch_referencing_prior_query_result_over_real_wire() {
    let (handle, client, _password) = boot().await;
    make_db(&client).await;

    let mut b = Batch::new();
    b.id("seed");
    b.insert(
        "ins",
        insert("users").rows([
            doc! { "name" => "heidi", "score" => 100_i64, "tier" => "vip" },
            doc! { "name" => "ivan", "score" => 30_i64, "tier" => "newbie" },
        ]),
    );
    client.execute("conddb", b.build()).await.expect("seed");

    // Batch: first query fetches heidi's tier value ("vip"). Second query's
    // WHERE-filter uses a $cond whose `then` branch is a $query reference to
    // that first query's result — i.e. the condition's THEN value is only
    // known after the batch planner resolves the cross-query data edge.
    let mut q = Batch::new();
    q.id("cross");
    let threshold = q.query(
        "threshold_lookup",
        Query::from("users").where_eq("name", "heidi"),
    );

    // Intended semantics (currently unreachable — see bug note above):
    // WHERE tier == cond(score >= 100, <tier of heidi row>, "newbie")
    // For heidi (score=100, tier="vip"): cond SHOULD evaluate to
    // threshold_lookup's tier ("vip"), matching heidi's own tier.
    // For ivan (score=30, tier="newbie"): cond evaluates to the literal
    // "newbie", which equals ivan's own tier -> he matches regardless of
    // the bug (his branch has no $query ref).
    let cross_cond = cond(
        gte("score", 100_i64),
        threshold.first().field("tier"),
        "newbie",
    );

    // Explicit `after` added defensively (harmless even though the planner
    // bug above means it doesn't grant data access on its own) so this test
    // exercises the best ordering guarantee available today.
    let rd = q.query_after(
        "rd",
        Query::from("users").where_eq("tier", cross_cond),
        &[&threshold],
    );
    let _ = &rd;

    let resp = client.execute("conddb", q.build()).await.expect("cross");

    let rd = resp.results.get("rd").expect("rd alias");
    let mut names: Vec<&str> = rd
        .records
        .iter()
        .filter_map(|r| r.get_value_str("name"))
        .collect();
    names.sort_unstable();

    // BUG-DOCUMENTING ASSERTION: `heidi` is missing today because the
    // $query-ref branch of her `$cond` silently resolves to `None` (the
    // dependency was never detected, so `threshold_lookup`'s result never
    // reaches `rd`'s FilterContext). Only `ivan` matches, whose branch is a
    // plain literal unaffected by the bug. When the planner bug is fixed,
    // this assertion should be updated to `vec!["heidi", "ivan"]`.
    assert_eq!(
        names,
        vec!["ivan"],
        "documents a known engine bug (see doc comment above this test): \
         extract_deps_from_filter_value does not recurse into \
         FilterValue::Cond, so a $query ref nested in a $cond branch never \
         becomes a DataFlow dependency edge, and the branch silently \
         resolves to None instead of heidi's tier. Got: {rd:?}"
    );

    client.close().await;
    handle.shutdown().await;
}
