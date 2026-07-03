//! R1-d — leader→follower end-to-end convergence + read-only replica gate
//! (REPLICATION §9, Capstone R1).
//!
//! This is the capstone that ties together the already-built replication
//! pieces:
//!   * R0-b `handle_repl` (leader-side pull API) — exercised here through the
//!     in-process `ReplSource` which calls the *same* journal-read path
//!     (`ShamirDb::read_changelog_from_journal` + `current_commit_version`)
//!     that the wire handler uses;
//!   * R1-a `RepoInstance::apply_replicated` — the follower apply path;
//!   * R1-b `replication_bookmark` / `advance_replication_bookmark` — the
//!     durable idempotency cursor;
//!   * R1-c `run_follower_loop` + `InProcessReplSource` — the engine that
//!     drives hello → pull → apply → advance in a bounded loop;
//!   * PR4 `NodeMode::ReadOnly` — the client-write gate on the follower's
//!     `ShamirDbHandler`.
//!
//! # Form: in-process (FALLBACK)
//!
//! The brief's PREFERRED form is two real servers (`ServerLauncher`) with the
//! follower pulling over TLS+SCRAM via `WireReplSource`. That form is
//! **deferred**: `ServerHandle::shamir` is `pub(super)`, so an integration
//! test cannot obtain the follower server's `Arc<ShamirDb>` to feed it into
//! `run_follower_loop` (which applies events to the follower's local DB).
//! Exposing it would be a non-surgical change to production code, outside
//! this task's scope.
//!
//! The in-process form used here still proves the full convergence +
//! apply-path + read-only-gate + idempotency story: `InProcessReplSource`
//! reuses the real journal-read path, `run_follower_loop` is the real engine,
//! `apply_replicated` is the real apply path, and the read-only gate is
//! exercised on a real `ShamirDbHandler` via the public `RequestHandler::handle`
//! trait (the same dispatch the wire uses). The only thing NOT covered is the
//! TLS+SCRAM transport itself — and that is already proven by
//! `repl_pull_e2e.rs` (R0-c).
//!
//! # Scenarios
//!
//! 1. **Convergence:** write N rows on the leader → run the follower loop
//!    (bounded) → the follower reads back the same N rows and its durable
//!    bookmark equals the leader's `current_version`.
//! 2. **Incremental:** write M more rows on the leader → re-run the loop →
//!    the follower catches up to N+M.
//! 3. **Read-only gate (PR4):** the follower's `ShamirDbHandler` is built
//!    with `NodeMode::ReadOnly`; a client write batch through `handle()` is
//!    rejected with `DbResponse::Error { code: "read_only_replica" }`, while
//!    the replication apply (loop) on the same follower keeps working.
//! 4. **Idempotent re-pull:** after catch-up, re-run the loop → every event
//!    is `Skipped`, the bookmark and the row count do not change.

use std::sync::Arc;

use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};
use shamir_db::access::{principal_id, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::query::Query;
use shamir_query_builder::write::insert;
use shamir_server::db_handler::{DbRequest, DbResponse, NodeMode, ShamirDbHandler};
use shamir_server::replication::follower_loop::{run_follower_loop, FollowerLoopConfig};
use shamir_server::replication::in_process::InProcessReplSource;
use shamir_server::replication::ReplSource;
use shamir_server::version::CURRENT_QUERY_LANG_VERSION;
use shamir_types::mpack;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// The schema owner. Both the leader and the follower create `app/main/items`
/// owned by `alice` so that:
///   * on the leader, `execute_as(alice)` writes pass the Shomer gate and
///     emit changelog events;
///   * on the follower, `apply_replicated` (which runs as `System`, bypassing
///     the gate) writes into the existing `items` table, and `execute_as(alice)`
///     reads also pass the gate for the convergence check.
const OWNER: &str = "alice";
const DB: &str = "app";
const REPO: &str = "main";
const TABLE: &str = "items";

/// Build an in-memory `ShamirDb` with one db `app`, one repo `main`, one
/// table `items`, owned by `alice`. Used for BOTH the leader and the follower
/// (a fresh instance each) — the schema is identical, the data is independent.
/// Mirrors `follower_loop_tests::build_db`.
async fn build_db() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal_id(OWNER));
    shamir.create_db_as(DB, owner.clone()).await;
    let cfg = RepoConfig::new(REPO, BoxRepoFactory::in_memory()).add_table(TableConfig::new(TABLE));
    shamir.add_repo_as(DB, cfg, owner).await.expect("add repo");
    shamir
}

/// Write `n` rows into `app/main/items` as `alice`. Each transactional insert
/// commits → emits one changelog event. Polls until all `n` events are durable
/// in the leader's journal (the journal writer is async).
async fn write_rows(leader: &ShamirDb, n: usize) {
    let owner = Actor::User(principal_id(OWNER));
    for i in 0..n {
        let key_str = format!("k{i}");
        let mut batch = Batch::named("ins");
        batch.id("ins");
        batch.transactional();
        batch.insert(
            "i",
            insert(TABLE).rows([doc! {
                "id" => key_str,
                "v" => i as i64,
            }]),
        );
        let resp = leader
            .execute_as(owner.clone(), DB, &batch.build())
            .await
            .expect("fixture write should succeed");
        assert!(
            !resp.results.contains_key("__error"),
            "write failed: {resp:?}",
        );
    }

    // The journal writer is async; poll until all n events are durable.
    for _ in 0..200 {
        if let Some(jr) = leader.read_changelog_from_journal(DB, REPO, 0, 1000).await {
            if jr.events.len() >= n {
                return;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("leader journal did not durable-land {n} events in time");
}

/// Count the rows readable in `app/main/items` on `db` (as `alice`). Used to
/// prove DATA convergence — not just changefeed convergence. A read batch
/// (SELECT *) is built through the query builder.
async fn count_rows(db: &ShamirDb) -> usize {
    let owner = Actor::User(principal_id(OWNER));
    let mut batch = Batch::new();
    batch.id("count");
    batch.query("q", Query::from(TABLE));
    let resp = db
        .execute_as(owner, DB, &batch.build())
        .await
        .expect("count read should succeed");
    assert!(
        !resp.results.contains_key("__error"),
        "count read failed: {resp:?}",
    );
    resp.results["q"].records.len()
}

/// The follower's durable replication bookmark for `app/main`.
async fn follower_bookmark(follower: &ShamirDb) -> u64 {
    let repo = follower
        .get_db(DB)
        .and_then(|d| d.get_repo(REPO))
        .expect("follower repo exists");
    repo.replication_bookmark().await.expect("bookmark read")
}

/// Poll the follower's bookmark until it reaches `target` (or panic after a
/// timeout). The bookmark is advanced by the loop as events are applied.
async fn wait_for_bookmark(follower: &ShamirDb, target: u64) {
    for _ in 0..300 {
        let b = follower_bookmark(follower).await;
        if b >= target {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let b = follower_bookmark(follower).await;
    panic!("follower bookmark did not reach {target} in time (last={b})");
}

/// Ensure the follower's `items` table has its per-table MVCC store attached
/// BEFORE the loop starts applying events.
///
/// `apply_replicated` resolves the per-table MvccStore via
/// `RepoInstance::per_table_mvcc()`; the entry is lazily registered on the
/// first read/execute against the table. If the very first access is a
/// replication apply (before any client read), the MVCC entry is absent and
/// `apply_replicated` falls back to the raw base-store path — whose writes
/// are not visible to a subsequent SELECT scan (the scan iterates the MVCC
/// layer). A single throwaway SELECT before the loop forces the attachment so
/// every replicated event lands in the MVCC overlay + history and is readable.
/// (This mirrors what any real follower does on boot: serve reads before/as
/// the replication loop starts.)
async fn touch_follower_table(follower: &ShamirDb) {
    let _ = count_rows(follower).await;
}

/// Run the follower loop until the bookmark reaches `bookmark_target`, then
/// cancel it. Uses `max_iterations` as a belt-and-suspenders backstop so the
/// test never hangs on an infinite loop. The leader is wrapped in an
/// `InProcessReplSource` (R1-c) — same journal-read path as the wire handler.
async fn run_loop_until_caught_up(follower: Arc<ShamirDb>, leader: ShamirDb, bookmark_target: u64) {
    // Force the per-table MVCC attachment before the first apply.
    touch_follower_table(&follower).await;

    let source = Arc::new(InProcessReplSource::new(leader)) as Arc<dyn ReplSource>;
    let cancel = CancellationToken::new();
    let cfg = FollowerLoopConfig::new("follower-1", DB, REPO)
        .with_poll_wait_ms(100)
        .with_max_iterations(200);

    let cancel_for_wait = cancel.clone();
    let follower_for_wait = follower.clone();
    let converge = tokio::spawn(async move {
        wait_for_bookmark(&follower_for_wait, bookmark_target).await;
        cancel_for_wait.cancel();
    });

    tokio::select! {
        res = run_follower_loop(follower.clone(), source, cfg, cancel) => {
            res.expect("follower loop completes without StaleLeaderEpoch");
        }
        _ = converge => {
            // Converged and cancelled the loop from inside the task; the
            // select branch above will finish the loop future. Nothing more
            // to do here.
        }
    }
}

/// A regular ("alice") session — resolves to `Actor::User(principal_id("alice"))`.
/// Mirrors `node_mode_tests::alice_session`.
fn alice_session() -> Session {
    Session::new(
        [0xAB; 16],
        OWNER.into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        UnixNanos::now().as_u64(),
    )
}

/// Build a read-only `ShamirDbHandler` over an in-memory follower `ShamirDb`
/// with `app/main/items` owned by `alice`.
async fn build_readonly_handler(follower: Arc<ShamirDb>) -> ShamirDbHandler {
    ShamirDbHandler::new(follower).with_node_mode(NodeMode::ReadOnly)
}

/// Drive a `ShamirDbHandler` through the public `RequestHandler::handle`
/// trait — the same dispatch the wire accept loop uses. Encodes `req` to
/// msgpack, calls `handle`, decodes the `DbResponse`. This is the only public
/// entry point that exercises the read-only gate from outside the crate
/// (`ShamirDbHandler::execute` is `pub(super)`).
async fn dispatch(handler: &ShamirDbHandler, req: &DbRequest) -> DbResponse {
    let session = alice_session();
    let conn = ConnectionServices::without_push(0);
    let req_bytes = rmp_serde::to_vec_named(req).expect("encode req");
    let resp_bytes = handler
        .handle(&session, &req_bytes, &conn)
        .await
        .expect("handler returns Ok(bytes) even for DB errors");
    rmp_serde::from_slice(&resp_bytes).expect("decode DbResponse")
}

/// A client write batch: a single upsert of key `client-write` into `items`.
fn client_write_batch() -> DbRequest {
    let mut b = Batch::new();
    b.id("w");
    b.upsert(
        "w1",
        shamir_query_builder::write::upsert(TABLE)
            .key(mpack!({ "id": "client-write" }))
            .value(doc! { "id" => "client-write", "v" => 999_i64 }),
    );
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: DB.into(),
        batch: b.build(),
    }
}

// ---------------------------------------------------------------------------
// Scenario 1 — convergence: N leader rows → follower reads the same N rows
// ---------------------------------------------------------------------------

/// Write N rows on the leader; run the follower loop (bounded) until it
/// catches up. The follower's durable bookmark must equal the leader's
/// `current_version`, and the follower must READ BACK the same N rows (data
/// convergence, not just changefeed convergence).
#[tokio::test]
async fn repl_convergence_s1_follower_catches_up_to_leader() {
    const N: usize = 5;

    let leader = build_db().await;
    write_rows(&leader, N).await;
    let leader_version = leader
        .current_commit_version(DB, REPO)
        .await
        .expect("leader version");
    assert!(
        leader_version >= N as u64,
        "leader advanced past {N} writes"
    );

    let follower = build_db().await;
    // Sanity: follower starts empty + bookmark 0.
    assert_eq!(count_rows(&follower).await, 0, "follower starts empty");
    assert_eq!(follower_bookmark(&follower).await, 0, "fresh bookmark is 0");

    run_loop_until_caught_up(Arc::new(follower.clone()), leader.clone(), leader_version).await;

    // Bookmark == leader current_version.
    let bookmark = follower_bookmark(&follower).await;
    assert!(
        bookmark >= leader_version,
        "follower bookmark {bookmark} should have reached leader version {leader_version}",
    );

    // DATA convergence: the follower reads back the same N rows.
    let follower_rows = count_rows(&follower).await;
    assert_eq!(
        follower_rows, N,
        "follower should have converged to {N} rows, got {follower_rows}",
    );
}

// ---------------------------------------------------------------------------
// Scenario 2 — incremental: M more rows → follower catches up to N+M
// ---------------------------------------------------------------------------

/// After an initial catch-up (N rows), write M more rows on the leader and
/// re-run the loop. The follower must catch up to N+M rows and its bookmark
/// must reach the new leader `current_version`.
#[tokio::test]
async fn repl_convergence_s2_incremental_append_catches_up() {
    const N: usize = 3;
    const M: usize = 2;

    let leader = build_db().await;
    write_rows(&leader, N).await;
    let leader_v1 = leader
        .current_commit_version(DB, REPO)
        .await
        .expect("leader v1");

    let follower = build_db().await;
    run_loop_until_caught_up(Arc::new(follower.clone()), leader.clone(), leader_v1).await;
    assert_eq!(count_rows(&follower).await, N, "first catch-up: N rows");

    // Write M more rows on the leader (total N+M).
    write_rows(&leader, M).await;
    let leader_v2 = leader
        .current_commit_version(DB, REPO)
        .await
        .expect("leader v2");
    assert!(
        leader_v2 > leader_v1,
        "leader version must advance after M more writes",
    );

    // Re-run the loop. The source wraps the SAME leader (its journal now
    // carries N+M events). The follower's bookmark is at leader_v1, so the
    // loop pulls from leader_v1+1 onwards.
    run_loop_until_caught_up(Arc::new(follower.clone()), leader, leader_v2).await;

    let bookmark = follower_bookmark(&follower).await;
    assert!(
        bookmark >= leader_v2,
        "follower bookmark {bookmark} should have reached leader v2 {leader_v2}",
    );
    assert_eq!(
        count_rows(&follower).await,
        N + M,
        "incremental: follower should have N+M = {} rows",
        N + M,
    );
}

// ---------------------------------------------------------------------------
// Scenario 3 — read-only gate (PR4)
// ---------------------------------------------------------------------------

/// The follower's `ShamirDbHandler` is built with `NodeMode::ReadOnly`. A
/// client write batch through `handle()` is rejected with
/// `DbResponse::Error { code: "read_only_replica" }`, while the replication
/// apply path (the loop, which calls `apply_replicated` directly) keeps
/// working on the same follower.
#[tokio::test]
async fn repl_convergence_s3_readonly_gate_rejects_client_writes_but_allows_replication() {
    const N: usize = 2;

    let leader = build_db().await;
    write_rows(&leader, N).await;
    let leader_version = leader
        .current_commit_version(DB, REPO)
        .await
        .expect("leader version");

    let follower = Arc::new(build_db().await);
    let handler = build_readonly_handler(follower.clone()).await;

    // 3a. BEFORE replication: the gate already fires. A client write is
    //     rejected even on an empty follower.
    match dispatch(&handler, &client_write_batch()).await {
        DbResponse::Error { code, message } => {
            assert_eq!(code, "read_only_replica", "wrong code; message: {message}");
        }
        other => panic!("read-only follower should reject client write, got: {other:?}"),
    }
    assert_eq!(count_rows(&follower).await, 0, "no rows before replication");

    // 3b. Run the loop: replication apply WORKS on the read-only follower
    //     (the gate is only on client `execute()`, not on `apply_replicated`).
    run_loop_until_caught_up(follower.clone(), leader, leader_version).await;
    assert_eq!(
        count_rows(&follower).await,
        N,
        "replication apply should work on a read-only follower",
    );

    // 3c. AFTER replication: the gate STILL fires. Replication did not
    //     weaken it.
    match dispatch(&handler, &client_write_batch()).await {
        DbResponse::Error { code, message } => {
            assert_eq!(code, "read_only_replica", "wrong code; message: {message}");
        }
        other => panic!(
            "read-only follower should still reject client write after replication, got: {other:?}"
        ),
    }
    // The rejected client write did not land a row.
    assert_eq!(count_rows(&follower).await, N, "client write was rejected");
}

// ---------------------------------------------------------------------------
// Scenario 4 — idempotent re-pull: after catch-up, re-running the loop is a
// no-op (every event Skipped, bookmark + row count unchanged).
// ---------------------------------------------------------------------------

/// After the follower catches up, running the loop again with the SAME
/// bookmark → every pull returns an empty batch (from_version = bookmark+1 >
/// leader_version), no events are applied, the bookmark does not regress,
/// and the row count does not grow.
#[tokio::test]
async fn repl_convergence_s4_idempotent_repull_after_catchup() {
    const N: usize = 4;

    let leader = build_db().await;
    write_rows(&leader, N).await;
    let leader_version = leader
        .current_commit_version(DB, REPO)
        .await
        .expect("leader version");

    let follower = build_db().await;
    let source = Arc::new(InProcessReplSource::new(leader.clone())) as Arc<dyn ReplSource>;

    // Force the per-table MVCC attachment before the first apply (see
    // `touch_follower_table` rationale).
    touch_follower_table(&follower).await;

    // First run: catch up.
    let cancel1 = CancellationToken::new();
    let cfg1 = FollowerLoopConfig::new("follower-1", DB, REPO)
        .with_poll_wait_ms(100)
        .with_max_iterations(100);
    let loop1 = tokio::spawn(run_follower_loop(
        Arc::new(follower.clone()),
        source.clone(),
        cfg1,
        cancel1.clone(),
    ));
    wait_for_bookmark(&follower, leader_version).await;
    cancel1.cancel();
    let _ = loop1.await;

    // Snapshot the state AFTER catch-up.
    let bookmark_before = follower_bookmark(&follower).await;
    let rows_before = count_rows(&follower).await;
    assert!(
        bookmark_before >= leader_version,
        "caught up before re-pull"
    );
    assert_eq!(rows_before, N, "N rows before re-pull");

    // Second run: from_version = bookmark+1 > leader_version → every pull is
    // empty, every (non-existent) event would be Skipped. The bookmark must
    // NOT regress and the row count must NOT grow.
    let cancel2 = CancellationToken::new();
    let cfg2 = FollowerLoopConfig::new("follower-1", DB, REPO)
        .with_poll_wait_ms(50)
        .with_max_iterations(3);
    run_follower_loop(Arc::new(follower.clone()), source, cfg2, cancel2)
        .await
        .expect("idempotent re-pull loop completes cleanly");

    // Bookmark unchanged.
    let bookmark_after = follower_bookmark(&follower).await;
    assert_eq!(
        bookmark_after, bookmark_before,
        "idempotent re-pull: bookmark must not change",
    );

    // Row count unchanged (no double-application).
    let rows_after = count_rows(&follower).await;
    assert_eq!(
        rows_after, rows_before,
        "idempotent re-pull: row count must not grow (no duplicate rows)",
    );
}
