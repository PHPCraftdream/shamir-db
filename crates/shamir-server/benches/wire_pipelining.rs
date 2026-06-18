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
//! Each bench function fires N independent `Execute` (one-shot transactional
//! upsert) requests as separate `tokio::spawn` tasks, then awaits all of them
//! via `try_join_all`. Total wall-clock for the N-burst is the per-iter cost;
//! `Throughput::Elements(N)` turns that into a txs/sec figure.
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
//! * Multi-thread runtime with ≥ 4 workers — real parallelism is required;
//!   a current-thread runtime would serialise all spawned tasks.

use shamir_types::mpack;
use shamir_types::types::value::QueryValue;
use std::sync::Arc;
use std::time::Instant;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use futures_util::future::join_all;

use shamir_bench_utils as bu;
use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};

use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::write::upsert;

use shamir_server::db_handler::{DbRequest, DbResponse, ShamirDbHandler};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

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

fn build_handler(rt: &tokio::runtime::Runtime) -> ShamirDbHandler {
    rt.block_on(async {
        let shamir = ShamirDb::init_memory().await.expect("init shamir");
        shamir.create_db("app").await;
        let cfg = RepoConfig::new("main", BoxRepoFactory::in_memory())
            .add_table(TableConfig::new("items"));
        shamir.add_repo("app", cfg).await.expect("add repo");
        ShamirDbHandler::new(Arc::new(shamir))
    })
}

/// Encode a one-shot transactional `Execute` carrying a single upsert of
/// key `k{i}`. Each call produces a fully independent payload so N callers
/// can dispatch in parallel without sharing a hot row.
/// Like [`execute_bytes_with_durability`] with `durability = None`.
#[allow(dead_code)]
fn execute_bytes(i: usize) -> Vec<u8> {
    execute_bytes_with_durability(i, None)
}

/// Like [`execute_bytes`] but with a wire-level `durability` knob — used by
/// the `async_index` group to compare against the default `Synchronous`
/// commit visibility (default `transactional` mode).
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

// ---------------------------------------------------------------------------
// Core bench driver
// ---------------------------------------------------------------------------

/// Fire `n` concurrent Execute requests and return the wall-clock elapsed.
///
/// Each task gets its own pre-encoded payload (built outside the timed
/// section) so encoding cost is excluded. The `handler` and `session` are
/// cloned-by-reference via `Arc` / cheap struct clone.
fn run_concurrent_burst(
    rt: &tokio::runtime::Runtime,
    handler: &ShamirDbHandler,
    session: &Session,
    payloads: &[Vec<u8>],
) {
    let n = payloads.len();
    rt.block_on(async {
        // Use join_all so all N futures are polled concurrently on the same
        // task. The `block_in_place` inside each `handle` call transparently
        // moves the blocking work to a thread-pool worker, so the multi-thread
        // runtime delivers real parallelism across the N requests.
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
    });
}

// ---------------------------------------------------------------------------
// Bench group
// ---------------------------------------------------------------------------

fn bench_wire_pipelining(c: &mut Criterion) {
    // ≥ 4 workers so N=128 tasks can run truly in parallel, not queued.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("rt");

    let handler = build_handler(&rt);
    let session = fixture_session();

    // Two groups: default (Synchronous commit visibility) and async_index
    // (Phase 5c+6.5+7 deferred to a tokio::spawn'd background task). The
    // pair side-by-side is the comparison that proves whether the
    // commit_mutex critical section shrink lifts the pipelining ceiling.
    for (mode_label, durability_str) in [("sync", None), ("async_index", Some("async_index"))] {
        let mut g = c.benchmark_group(format!("wire_pipelining/{mode_label}"));

        for &n in &[1_usize, 8, 32, 128] {
            let payloads: Vec<Vec<u8>> = (0..n)
                .map(|i| execute_bytes_with_durability(i, durability_str))
                .collect();

            g.throughput(Throughput::Elements(n as u64));
            bu::tune(&mut g, 30, 5, 3);

            g.bench_function(format!("n_{n}"), |b| {
                b.iter_custom(|iters| {
                    let mut total = std::time::Duration::ZERO;
                    for _ in 0..iters {
                        let start = Instant::now();
                        run_concurrent_burst(&rt, &handler, &session, &payloads);
                        total += start.elapsed();
                    }
                    total
                });
            });
        }

        g.finish();
    }
}

criterion_group!(benches, bench_wire_pipelining);
criterion_main!(benches);
