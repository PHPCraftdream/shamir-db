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
/// # Bug #642 — fixed in Epic03/B (`997532cc`), assertion updated
///
/// This test originally documented a known engine bug: `$query` refs
/// nested inside a `$cond` branch (or `$expr`/`FnCall` args) were not
/// recognized by `BatchPlanner::extract_deps_from_filter_value`
/// (`crates/shamir-query-types/src/batch/planner.rs`), so `heidi`'s
/// `$cond`-branch `$query`-ref onto `threshold_lookup` silently resolved
/// to `None` and she was wrongly excluded from `rd`'s results. Bug #642
/// added the missing `Cond`/`Expr`/`FnCall` recursion arms as part of
/// Epic03/B's `when` implementation (both WHERE clauses and `when` share
/// this one leaf extractor). The dependency is now correctly detected as
/// `EdgeKind::DataFlow`, `threshold_lookup`'s result reaches `rd`'s
/// `resolved_refs`, and `heidi` now matches as originally intended.
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

    // WHERE tier == cond(score >= 100, <tier of heidi row>, "newbie")
    // For heidi (score=100, tier="vip"): cond evaluates to
    // threshold_lookup's tier ("vip"), matching heidi's own tier.
    // For ivan (score=30, tier="newbie"): cond evaluates to the literal
    // "newbie", which equals ivan's own tier -> he matches too.
    let cross_cond = cond(
        gte("score", 100_i64),
        threshold.first().field("tier"),
        "newbie",
    );

    // Explicit `after` is redundant now (the $query ref inside $cond is
    // correctly auto-detected as a DataFlow dependency, #642) but kept for
    // clarity/defense-in-depth ordering.
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

    // Both match now that bug #642 is fixed: heidi's $cond branch correctly
    // resolves threshold_lookup's tier ("vip") via the now-detected DataFlow
    // dependency; ivan matches via the plain literal "newbie" branch.
    assert_eq!(
        names,
        vec!["heidi", "ivan"],
        "expected both heidi (via the $query-ref $cond branch, now fixed by \
         #642) and ivan (via the literal branch) to match. Got: {rd:?}"
    );

    client.close().await;
    handle.shutdown().await;
}
