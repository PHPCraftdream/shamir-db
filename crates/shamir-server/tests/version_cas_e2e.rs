//! FG-2 e2e: `with_version` / `expected_version` optimistic-concurrency
//! (CAS) contour through a REAL server + the high-level `shamir_client::Client`.
//!
//! Mirrors `quickstart_e2e.rs`'s connect/execute shape and
//! `crates/shamir-engine/src/table/tests/version_cas_tests.rs`'s concurrent
//! CAS scenario, but drives it end-to-end over the wire: two real client
//! connections race an UPDATE with the SAME `expected_version`; exactly one
//! must succeed and the other must see a `version_conflict` typed error;
//! then a retry with the freshly-read version must succeed.

use shamir_types::mpack;
use tempfile::TempDir;
use zeroize::Zeroizing;

use shamir_client::{Client, ClientError, ConnectOptions};
use shamir_query_builder::batch::{Batch, Isolation};
use shamir_query_builder::filter;
use shamir_query_builder::write::{update, upsert};
use shamir_query_builder::{doc, Query};

mod common;

async fn connect_admin(addr: std::net::SocketAddr, admin_pw: &[u8]) -> Client {
    Client::connect(ConnectOptions {
        addr,
        server_name: "localhost".to_string(),
        username: "admin".to_string(),
        password: Zeroizing::new(admin_pw.to_vec()),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: None,
        request_timeout: None,
    })
    .await
    .expect("connect")
}

/// Read the current `versions[0]` for the single row matching `id`.
async fn read_version(client: &Client, id: &str) -> u64 {
    let mut b = Batch::new();
    b.id("r");
    b.query(
        "g",
        Query::from("kv")
            .where_(filter::eq("id", id))
            .with_version(),
    );
    let resp = client.execute("default", b.build()).await.expect("read");
    let result = &resp.results["g"];
    assert_eq!(result.records.len(), 1, "exactly one row for {id}");
    result
        .versions
        .as_ref()
        .expect("versions must be Some when with_version() is set")[0]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_version_read_and_expected_version_write_e2e() {
    let temp = TempDir::new().expect("tempdir");
    let admin_pw = b"change-me-admin".to_vec();
    let handle = common::spawn_ephemeral(&temp, &admin_pw).await;
    let addr = handle.first_tls_exporter_addr().expect("bound");

    let client = connect_admin(addr, &admin_pw).await;

    // Create table + seed a row in the pre-existing default/main store.
    let mut mk_batch = Batch::new();
    mk_batch.id("mk");
    mk_batch.create_table("t", shamir_query_builder::ddl::create_table("kv"));
    client
        .execute("default", mk_batch.build())
        .await
        .expect("create_table");

    let mut put_b = Batch::new();
    put_b.id("put");
    put_b.upsert(
        "p",
        upsert("kv")
            .key(mpack!({"id": "row1"}))
            .value(doc! { "id" => "row1", "val" => 1 }),
    );
    client.execute("default", put_b.build()).await.expect("put");

    // Read-side: with_version() must return a version for the seeded row.
    let v0 = read_version(&client, "row1").await;

    // Write-side: expected_version(v0) must succeed and bump the version.
    let mut up_b = Batch::new();
    up_b.id("u");
    up_b.update(
        "u1",
        update("kv")
            .where_(filter::eq("id", "row1"))
            .set(doc! { "val" => 2 })
            .expected_version(v0),
    );
    client
        .execute("default", up_b.build())
        .await
        .expect("expected_version match must succeed");

    let v1 = read_version(&client, "row1").await;
    assert!(v1 > v0, "version must increase after update: {v0} -> {v1}");

    // Stale expected_version (v0, now outdated) must be rejected with the
    // typed `version_conflict` code — no row modified.
    let mut stale_b = Batch::new();
    stale_b.id("s");
    stale_b.update(
        "u2",
        update("kv")
            .where_(filter::eq("id", "row1"))
            .set(doc! { "val" => 999 })
            .expected_version(v0),
    );
    let err = client
        .execute("default", stale_b.build())
        .await
        .expect_err("stale expected_version must be rejected");
    match err {
        ClientError::Db { code, .. } => assert_eq!(code, "version_conflict"),
        other => panic!("expected ClientError::Db(version_conflict), got: {other:?}"),
    }

    // Row unchanged after the rejected attempt.
    let mut check_b = Batch::new();
    check_b.id("c");
    check_b.query("g", Query::from("kv").where_(filter::eq("id", "row1")));
    let resp = client
        .execute("default", check_b.build())
        .await
        .expect("read after rejected CAS");
    let rows = &resp.results["g"].records;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_value_i64("val"), Some(2), "row unchanged");

    handle.shutdown().await;
}

/// CONCURRENT CAS through the real server: two independent client
/// connections both read the same row's version, then race an UPDATE with
/// that SAME `expected_version`. Exactly one must succeed; the other must
/// fail with `version_conflict`. A retry with the fresh version then
/// succeeds.
///
/// Both racing tasks share ONE already-authenticated connection
/// (`Arc<Client>`) rather than opening a second/third fresh SCRAM
/// handshake — `Client::execute` takes `&self` and multiplexes concurrent
/// requests over the same socket via per-request ids, so this is still a
/// genuine two-real-concurrent-request race at the wire/commit level, not a
/// sequential fake. Opening additional fresh connections here would trip
/// the server's per-subnet `auth_init` rate limiter's restart-warmup
/// divisor (spec §8.6: capacity throttled to a handful of tokens for 60s
/// after boot) — a real, intentional anti-DoS behavior, not something
/// this test should work around by disabling server security config.
///
/// FG-7 FIX (this test used to document a real, open gap — now closed):
/// previously, only the `.transactional().isolation(Serializable)` variant
/// below closed the race; a PLAIN non-transactional `client.execute(...)`
/// carrying `expected_version` ran through
/// `RepoInstance::run_implicit_batch_tx`, hardcoded to
/// `IsolationLevel::Snapshot`, and `TxContext::record_read_shared` (the old
/// commit-time backstop) was a documented no-op outside `Serializable` — so
/// two concurrent CAS writers on the plain path could BOTH commit,
/// confirmed reproducible via repeated runs of this test body. FG-7
/// replaced that isolation-gated backstop with an independent `cas_set`
/// validated at commit UNCONDITIONALLY of isolation level, so "exactly one
/// wins" now holds on the plain non-transactional path too — proven below
/// alongside the pre-existing `Serializable` proof.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_cas_via_real_server_exactly_one_wins() {
    let temp = TempDir::new().expect("tempdir");
    let admin_pw = b"change-me-admin".to_vec();
    let handle = common::spawn_ephemeral(&temp, &admin_pw).await;
    let addr = handle.first_tls_exporter_addr().expect("bound");

    let setup_client = std::sync::Arc::new(connect_admin(addr, &admin_pw).await);

    let mut mk_batch = Batch::new();
    mk_batch.id("mk");
    mk_batch.create_table("t", shamir_query_builder::ddl::create_table("kv"));
    setup_client
        .execute("default", mk_batch.build())
        .await
        .expect("create_table");

    let mut put_b = Batch::new();
    put_b.id("put");
    put_b.upsert(
        "p",
        upsert("kv")
            .key(mpack!({"id": "counter"}))
            .value(doc! { "id" => "counter", "val" => 0 }),
    );
    setup_client
        .execute("default", put_b.build())
        .await
        .expect("put");

    let v0 = read_version(&setup_client, "counter").await;

    // Two concurrent tokio tasks racing the SAME shared connection.
    let client_a = setup_client.clone();
    let client_b = setup_client.clone();

    let task_a = tokio::spawn(async move {
        let mut b = Batch::new();
        b.id("a");
        b.update(
            "ua",
            update("kv")
                .where_(filter::eq("id", "counter"))
                .set(doc! { "val" => 100 })
                .expected_version(v0),
        );
        b.transactional().isolation(Isolation::Serializable);
        client_a.execute("default", b.build()).await
    });
    let task_b = tokio::spawn(async move {
        let mut b = Batch::new();
        b.id("b");
        b.update(
            "ub",
            update("kv")
                .where_(filter::eq("id", "counter"))
                .set(doc! { "val" => 200 })
                .expected_version(v0),
        );
        b.transactional().isolation(Isolation::Serializable);
        client_b.execute("default", b.build()).await
    });

    let (res_a, res_b) = tokio::join!(task_a, task_b);
    let res_a = res_a.expect("task_a join");
    let res_b = res_b.expect("task_b join");

    // A TRANSACTIONAL batch reports an abort in up to THREE shapes,
    // depending on WHICH check caught it (all are "this writer lost the
    // race" outcomes):
    //   - the immediate `version_of` check (step 1) rejects the op before
    //     any commit is attempted AND before the batch's own plan/stage
    //     phase even finishes — surfaces as a top-level
    //     `Err(ClientError::Db { code: "version_conflict", .. })`;
    //   - that SAME immediate check can ALSO surface as a per-batch
    //     STAGING-time abort still wrapped in `Ok(BatchResponse)` with
    //     `transaction.status == "aborted"` and a `reason` string that
    //     debug-formats the underlying `BatchError::QueryError { ...,
    //     code: Some("version_conflict") }` (see
    //     `crates/shamir-engine/src/query/batch/batch_execute.rs`'s
    //     `Err(plan_err) => TransactionInfo::aborted(tx_id, format!("{:?}",
    //     plan_err))` arm) — this fires when the OTHER writer's commit
    //     already bumped the version before this writer's own staging ran;
    //   - FG-7's new commit-time Phase CAS backstop (step 2) catches a
    //     concurrent writer AT COMMIT time (both writers' immediate checks
    //     passed, the race closed at commit instead), which ALSO surfaces
    //     as `Ok(BatchResponse)` with `transaction.status == "aborted"`,
    //     but with `reason` containing `"version_conflict"` (the NEW
    //     `CommitError::CasConflict` mapping — distinct from the old
    //     generic SSI `"tx_conflict"` a non-CAS Serializable conflict would
    //     produce). Checking for the `"version_conflict"` substring in
    //     `reason` covers both of the last two shapes without depending on
    //     the exact enclosing error-struct debug format.
    // Any of the three shapes is an acceptable "this writer lost the race"
    // outcome.
    fn lost_the_race(res: &Result<shamir_client::BatchResponse, ClientError>) -> bool {
        match res {
            Err(ClientError::Db { code, .. }) => code == "version_conflict",
            Ok(resp) => resp.transaction.as_ref().is_some_and(|t| {
                t.status == "aborted"
                    && t.reason
                        .as_deref()
                        .is_some_and(|r| r.contains("version_conflict") || r == "tx_conflict")
            }),
            _ => false,
        }
    }
    fn committed(res: &Result<shamir_client::BatchResponse, ClientError>) -> bool {
        matches!(
            res,
            Ok(resp) if resp.transaction.as_ref().is_none_or(|t| t.status == "committed")
        )
    }

    let a_ok = committed(&res_a);
    let b_ok = committed(&res_b);
    let a_conflict = lost_the_race(&res_a);
    let b_conflict = lost_the_race(&res_b);

    assert!(
        (a_ok && b_conflict) || (b_ok && a_conflict),
        "expected exactly one commit success and one version_conflict/tx_conflict abort, \
         got: a_ok={a_ok} b_ok={b_ok} a_conflict={a_conflict} b_conflict={b_conflict} \
         res_a={res_a:?} res_b={res_b:?}"
    );

    // Retry with the fresh version must succeed.
    let v1 = read_version(&setup_client, "counter").await;
    assert!(v1 > v0, "version must have advanced: {v0} -> {v1}");

    let mut retry_b = Batch::new();
    retry_b.id("retry");
    retry_b.update(
        "ur",
        update("kv")
            .where_(filter::eq("id", "counter"))
            .set(doc! { "val" => 999 })
            .expected_version(v1),
    );
    setup_client
        .execute("default", retry_b.build())
        .await
        .expect("retry with fresh version must succeed");

    // FG-7 (mandatory test #1): the SAME "exactly one wins" guarantee, now
    // ALSO proven on the PLAIN non-transactional path (no `.transactional()`
    // at all) — this is the path the doc comment above describes as
    // formerly broken. `client.execute(...)` with no transactional wrapper
    // routes through `RepoInstance::run_implicit_batch_tx`
    // (`IsolationLevel::Snapshot`); a commit-time CAS loser here surfaces as
    // a top-level `Err(ClientError::Db { code: "version_conflict", .. })`
    // (there is no `TransactionInfo` wrapper on a non-transactional batch),
    // identically to the immediate staging-time check.
    let v2 = read_version(&setup_client, "counter").await;
    let client_c = setup_client.clone();
    let client_d = setup_client.clone();

    let task_c = tokio::spawn(async move {
        let mut b = Batch::new();
        b.id("c");
        b.update(
            "uc",
            update("kv")
                .where_(filter::eq("id", "counter"))
                .set(doc! { "val" => 300 })
                .expected_version(v2),
        );
        // No `.transactional()` at all — plain non-tx batch.
        client_c.execute("default", b.build()).await
    });
    let task_d = tokio::spawn(async move {
        let mut b = Batch::new();
        b.id("d");
        b.update(
            "ud",
            update("kv")
                .where_(filter::eq("id", "counter"))
                .set(doc! { "val" => 400 })
                .expected_version(v2),
        );
        client_d.execute("default", b.build()).await
    });

    let (res_c, res_d) = tokio::join!(task_c, task_d);
    let res_c = res_c.expect("task_c join");
    let res_d = res_d.expect("task_d join");

    fn plain_path_lost_the_race(res: &Result<shamir_client::BatchResponse, ClientError>) -> bool {
        matches!(res, Err(ClientError::Db { code, .. }) if code == "version_conflict")
    }
    fn plain_path_committed(res: &Result<shamir_client::BatchResponse, ClientError>) -> bool {
        res.is_ok()
    }

    let c_ok = plain_path_committed(&res_c);
    let d_ok = plain_path_committed(&res_d);
    let c_conflict = plain_path_lost_the_race(&res_c);
    let d_conflict = plain_path_lost_the_race(&res_d);

    assert!(
        (c_ok && d_conflict) || (d_ok && c_conflict),
        "expected exactly one commit success and one version_conflict abort on the \
         PLAIN non-transactional path, got: c_ok={c_ok} d_ok={d_ok} \
         c_conflict={c_conflict} d_conflict={d_conflict} res_c={res_c:?} res_d={res_d:?}"
    );

    let v3 = read_version(&setup_client, "counter").await;
    assert!(
        v3 > v2,
        "version must have advanced after the winning plain-path commit: {v2} -> {v3}"
    );

    handle.shutdown().await;
}
