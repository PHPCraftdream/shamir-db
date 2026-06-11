//! Wire-path latency benches — Д2 (interactive tx lifecycle) and
//! Д3 (SCRAM handshake + resume fast-path).
//!
//! ## What this measures
//!
//! Group 1 — `interactive_tx_lifecycle` — drives the full
//! `tx_begin` → `tx_execute` → `tx_commit` round-trip through
//! `ShamirDbHandler::handle()` with msgpack-encoded `DbRequest`s.
//! No socket, no TLS, no Argon2 — same in-process harness as
//! `tests/interactive_tx_e2e.rs`. Closes the gap left by
//! `db_handler_rps.rs`, which only benches one-shot `Execute`.
//!
//! Three functions:
//!   * `begin_only` — open a tx and immediately drop the handle.
//!     Bounded cost of `TxBegin` dispatch + registry insert.
//!   * `begin_execute_commit_minimal` — begin + one trivial insert + commit.
//!     Minimum non-trivial tx lifecycle.
//!   * `begin_execute_n_commit` — same shape but 10 inserts in the
//!     execute step. Shows how lifecycle overhead amortizes over work.
//!
//! ## Group 2 — DEFERRED
//!
//! `handshake_paths` (full SCRAM connect vs ticket-based resume) was
//! intentionally not shipped here. It requires spawning a live
//! `target/release/shamir-server` process, provisioning a user,
//! issuing a resumption ticket, and tearing down — there is no
//! existing in-tree helper that exposes those pieces to a bench
//! crate (the `tests/*_e2e.rs` files inline ~100 LOC of setup each
//! against `cmd::server::run_server` and are not reusable from a
//! `[[bench]]` target). Per the task brief: "If spawning a server
//! from the bench proves architecturally fiddly … ship only Group 1".
//! Group 1 alone closes a real, previously-unmeasured gap (interactive
//! transactions); Group 2 belongs in a follow-up that adds a reusable
//! `tests/common/live_server.rs` helper first.

use std::sync::Arc;

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use serde_json::json;

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
// Fixtures (mirror tests/interactive_tx_e2e.rs)
// ---------------------------------------------------------------------------

fn fixture_session() -> Session {
    let mut s = Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        UnixNanos::now().as_u64(),
    );
    s.session_id = [0x11; 32];
    s
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

fn encode(req: &DbRequest) -> Vec<u8> {
    rmp_serde::to_vec_named(req).expect("encode req")
}

fn decode(bytes: &[u8]) -> DbResponse {
    rmp_serde::from_slice(bytes).expect("decode response")
}

fn tx_begin_bytes() -> Vec<u8> {
    encode(&DbRequest::TxBegin {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: "app".into(),
        repo: "main".into(),
        isolation: None,
    })
}

/// Build a `TxExecute` payload with `n` upserts via the typed batch
/// builder — no raw JSON for the query shape itself.
fn tx_execute_bytes(tx_handle: u64, n: usize) -> Vec<u8> {
    let mut b = Batch::new();
    b.id("w");
    b.return_only(std::iter::empty::<String>());
    for i in 0..n {
        let id = format!("k{i}");
        b.upsert(
            format!("s{i}"),
            upsert("items")
                .key(json!({ "id": id }))
                .value(doc! { "id" => id.clone(), "qty" => i as i64 }),
        );
    }
    encode(&DbRequest::TxExecute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: "app".into(),
        tx_handle,
        batch: b.build(),
    })
}

fn tx_commit_bytes(tx_handle: u64) -> Vec<u8> {
    encode(&DbRequest::TxCommit {
        db: "app".into(),
        tx_handle,
    })
}

fn tx_rollback_bytes(tx_handle: u64) -> Vec<u8> {
    encode(&DbRequest::TxRollback {
        db: "app".into(),
        tx_handle,
    })
}

/// Drive `handle()` once and return the decoded response.
async fn call(handler: &ShamirDbHandler, session: &Session, bytes: &[u8]) -> DbResponse {
    let raw = handler
        .handle(session, bytes, &ConnectionServices::without_push(0))
        .await
        .expect("handle ok");
    decode(&raw)
}

/// Extract a `tx_handle` from a `TxOpened` response.
fn expect_tx_handle(resp: DbResponse) -> u64 {
    match resp {
        DbResponse::TxOpened { tx_handle, .. } => tx_handle,
        other => panic!("expected TxOpened, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Group 1: interactive_tx_lifecycle
// ---------------------------------------------------------------------------

fn bench_interactive_tx(c: &mut Criterion) {
    // Multi-thread runtime: `ShamirDbHandler::handle` uses block_in_place.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("rt");

    let handler = build_handler(&rt);
    let session = fixture_session();

    let mut g = c.benchmark_group("interactive_tx_lifecycle");
    g.throughput(Throughput::Elements(1));

    // -- begin_only: cost of opening a tx, then rolling back so the
    //    registry stays empty. We must drain the handle to avoid an
    //    unbounded leak across thousands of iterations.
    {
        let begin_bytes = tx_begin_bytes();
        g.bench_function("begin_only", |b| {
            b.iter(|| {
                rt.block_on(async {
                    let resp = call(&handler, &session, black_box(&begin_bytes)).await;
                    let tx = expect_tx_handle(resp);
                    // Drop the tx via rollback — cheap and keeps the
                    // registry from growing. The bench targets BEGIN
                    // cost; the rollback adds a constant offset.
                    let rb = tx_rollback_bytes(tx);
                    let r = call(&handler, &session, &rb).await;
                    black_box(r);
                });
            });
        });
    }

    // -- begin_execute_commit_minimal: 1 upsert.
    {
        let begin_bytes = tx_begin_bytes();
        g.bench_function("begin_execute_commit_minimal", |b| {
            b.iter(|| {
                rt.block_on(async {
                    let tx =
                        expect_tx_handle(call(&handler, &session, black_box(&begin_bytes)).await);
                    let exec = tx_execute_bytes(tx, 1);
                    let r1 = call(&handler, &session, &exec).await;
                    black_box(&r1);
                    let c1 = tx_commit_bytes(tx);
                    let r2 = call(&handler, &session, &c1).await;
                    black_box(r2);
                });
            });
        });
    }

    // -- begin_execute_n_commit: 10 upserts in a single execute.
    {
        let begin_bytes = tx_begin_bytes();
        g.bench_function("begin_execute_n_commit", |b| {
            b.iter(|| {
                rt.block_on(async {
                    let tx =
                        expect_tx_handle(call(&handler, &session, black_box(&begin_bytes)).await);
                    let exec = tx_execute_bytes(tx, 10);
                    let r1 = call(&handler, &session, &exec).await;
                    black_box(&r1);
                    let c1 = tx_commit_bytes(tx);
                    let r2 = call(&handler, &session, &c1).await;
                    black_box(r2);
                });
            });
        });
    }

    g.finish();
}

criterion_group!(benches, bench_interactive_tx);
criterion_main!(benches);
