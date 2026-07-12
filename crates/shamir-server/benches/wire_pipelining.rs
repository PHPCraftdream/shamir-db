//! Wire-path pipelining throughput bench — measures concurrent in-flight
//! `Execute` requests on the in-process `ShamirDbHandler`.
//!
//! ## What this measures
//!
//! [`wire_latencies`] measures *sequential* latency (one round-trip at a
//! time, ~580 tx/s). This bench answers the complementary question:
//!
//! > If N requests are dispatched concurrently — all in flight at once —
//! > how many transactions per second does the handler sustain?
//!
//! Each variant fires N independent `Execute` (one-shot transactional
//! upsert) requests as separate `tokio::spawn` tasks, then awaits all of them
//! via `join_all`. Total wall-clock for the N-burst is the timed workload.
//!
//! ## What to expect
//!
//! * **Linear scaling** (`n_8` ≈ 8× `n_1`, `n_32` ≈ 32×, …) means the
//!   engine is truly lock-free on the write path and the runtime fan-out is
//!   effective — the bench is confirmation that pipelining works.
//!
//! * **Sub-linear but rising** — partial contention somewhere (e.g. a
//!   per-database write mutex). Still worthwhile; profile to narrow it.
//!
//! * **Flat across N** — a global serialisation point caps throughput
//!   regardless of concurrency. The first suspect is the MVCC commit path;
//!   look for a `Mutex` or `block_in_place` that serialises all writers.
//!
//! ## Key design choices
//!
//! * `Execute` (one-shot, 1 RT) rather than `TxBegin/TxExecute/TxCommit`
//!   (3 RTs) — we want to saturate the *handler* throughput, not the
//!   interactive-tx protocol overhead.
//! * `transactional: true` — real MVCC path, not a dry-run.
//! * Disjoint keys (`k{i}`) — avoid hot-row contention masking the
//!   concurrency win.
//! * The harness's shared multi-thread runtime gives real parallelism for
//!   the spawned tasks.

use std::hint::black_box;
use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

use futures_util::future::join_all;

use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};

use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::write::upsert;

use shamir_server::db_handler::{DbRequest, DbResponse, ShamirDbHandler};

include!("bench_allocator.rs");

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn fixture_session() -> Session {
    Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        UnixNanos::now().as_u64(),
    )
}

async fn build_handler() -> Arc<ShamirDbHandler> {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    // `create_db`/`add_repo` (System-owned) persist ResourceMeta::owned_enforced
    // (owner-only 0o700) rather than the old open 0o777 default. `fixture_session()`
    // above is a regular ("alice") session, not a superuser, so it resolves to
    // Actor::User(principal64([0xAB; 16])) and needs ownership to pass the gate.
    let bench_user = Actor::User(principal64([0xAB; 16]));
    shamir.create_db_as("app", bench_user.clone()).await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, bench_user)
        .await
        .expect("add repo");
    Arc::new(ShamirDbHandler::new(Arc::new(shamir)))
}

/// Encode a one-shot transactional `Execute` carrying a single upsert of
/// key `k{i}`. Each call produces a fully independent payload so N callers
/// can dispatch in parallel without sharing a hot row.
fn execute_bytes_with_durability(i: usize, durability: Option<&str>) -> Vec<u8> {
    let id = format!("k{i}");
    let mut b = Batch::new();
    b.transactional();
    if let Some(d) = durability {
        match d {
            "async_index" => {
                b.durability(shamir_query_builder::batch::Durability::AsyncIndex);
            }
            "synced" => {
                b.durability(shamir_query_builder::batch::Durability::Synced);
            }
            other => panic!("unknown durability: {other}"),
        }
    }
    b.upsert(
        format!("w{i}"),
        upsert("items")
            .key(mpack!({ "id": @(QueryValue::from(id.as_str())) }))
            .value(doc! { "id" => id.clone(), "v" => i as i64 }),
    );
    let req = DbRequest::Execute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: "app".into(),
        batch: b.build(),
    };
    rmp_serde::to_vec_named(&req).expect("encode execute")
}

/// Assert that a decoded `DbResponse` is `Batch` (i.e. the Execute succeeded).
fn assert_batch_ok(resp: DbResponse, i: usize) {
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "Execute {i} returned unexpected response: {resp:?}",
    );
}

/// Fire `n` concurrent Execute requests (one per `payloads` entry) and
/// await them all.
///
/// Each task gets its own pre-encoded payload (built outside the timed
/// section) so encoding cost is excluded.
async fn run_concurrent_burst(handler: &ShamirDbHandler, session: &Session, payloads: &[Vec<u8>]) {
    let n = payloads.len();
    // Use join_all so all N futures are polled concurrently on the same
    // task. The `block_in_place` inside each `handle` call transparently
    // moves the blocking work to a thread-pool worker, so the shared
    // multi-thread runtime delivers real parallelism across the N requests.
    let futs: Vec<_> = payloads
        .iter()
        .enumerate()
        .map(|(i, bytes)| {
            let conn = ConnectionServices::without_push(0);
            async move {
                let raw = handler
                    .handle(session, bytes, &conn)
                    .await
                    .unwrap_or_else(|e| panic!("Execute {i} handle error: {e}"));
                let resp: DbResponse = rmp_serde::from_slice(&raw).expect("decode response");
                assert_batch_ok(resp, i);
            }
        })
        .collect();

    // Await all N futures concurrently — silent failures panic loudly above.
    let results: Vec<()> = join_all(futs).await;
    assert_eq!(results.len(), n, "expected {n} responses");
}

fn setup_block_on<T>(fut: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(fut)
}

fn main() {
    let mut h = Harness::new("wire_pipelining", env!("CARGO_MANIFEST_DIR"));

    let handler = setup_block_on(build_handler());
    let session = Arc::new(fixture_session());

    // Two groups: default (Synchronous commit visibility) and async_index
    // (Phase 5c+6.5+7 deferred to a tokio::spawn'd background task). The
    // pair side-by-side is the comparison that proves whether the
    // commit_mutex critical section shrink lifts the pipelining ceiling.
    //
    // N = concurrent in-flight requests is a genuine structural axis
    // (pipelining: does throughput scale with concurrency?). Default =
    // smallest tier only (n=1, a single Execute — cheap); set
    // BENCH_WIRE_PIPELINING_SCALING=1 to run the full ladder.
    let wide = std::env::var("BENCH_WIRE_PIPELINING_SCALING")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);
    let ns: &[usize] = if wide {
        &[1_usize, 8, 32, 128]
    } else {
        &[1_usize]
    };
    for (mode_label, durability_str) in [("sync", None), ("async_index", Some("async_index"))] {
        for &n in ns {
            let payloads: Vec<Vec<u8>> = (0..n)
                .map(|i| execute_bytes_with_durability(i, durability_str))
                .collect();
            let payloads = Arc::new(payloads);

            let handler = handler.clone();
            let session = session.clone();
            let id = format!("wire_pipelining/{mode_label}/n_{n}");
            h.bench_async(&id, move || {
                let handler = handler.clone();
                let session = session.clone();
                let payloads = payloads.clone();
                async move {
                    run_concurrent_burst(&handler, &session, &payloads).await;
                    black_box(());
                }
            });
        }
    }

    h.run();
}
