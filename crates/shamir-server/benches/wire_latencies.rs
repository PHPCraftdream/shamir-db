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
//! ## Group 2 — `handshake_paths`
//!
//! Two bench functions that drive a fresh in-process live server via
//! the `tests/common/`-equivalent helper mirrored to `benches/common.rs`
//! (Cargo `[[bench]]` targets cannot reach into `tests/`):
//!
//!   * `full_scram_connect` — `Client::connect()` against a freshly
//!     spawned server. Dominated by Argon2id (memory_kb=19456, t=2,
//!     p=1 — spec floor). `sample_size(10)` to keep wall-clock bounded.
//!   * `resume_fast_path` — open ONCE in setup to capture a resumption
//!     ticket + pin, then bench `Client::resume()` in the loop. Skips
//!     Argon2id entirely; default `sample_size(100)` is fine.

use std::sync::Arc;

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use serde_json::json;

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

use shamir_client::{Client, ConnectOptions, ResumeOptions};
use tempfile::TempDir;
use zeroize::Zeroizing;

#[path = "common.rs"]
mod common;

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

// ---------------------------------------------------------------------------
// Group 2: handshake_paths — full SCRAM connect vs resumption fast-path
// ---------------------------------------------------------------------------

/// Wall-clock setup that spawns a live server and returns
/// `(handle, addr, tempdir, password)`. The `TempDir` is returned so its
/// lifetime extends across the whole bench — dropping it deletes the
/// data dir out from under the running server.
async fn setup_live_server() -> (
    shamir_server::server::ServerHandle,
    std::net::SocketAddr,
    TempDir,
    Vec<u8>,
) {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let temp = TempDir::new().expect("tempdir");
    let password = b"bench-admin-password".to_vec();

    // Bench-only: bump the per-subnet auth_init rate limit well above
    // the 10/s default so iterations don't get throttled.
    let mut config = common::make_test_config(&temp, "127.0.0.1:0");
    config.security.auth_init_rate_per_second = 100_000;

    let handle = shamir_server::server::ServerLauncher {
        config,
        bootstrap: shamir_server::server::BootstrapMode::Password {
            username: "admin".into(),
            password: Zeroizing::new(password.clone()),
        },
    }
    .launch()
    .await
    .expect("launcher boot");
    let addr = handle.first_tls_exporter_addr().expect("bound");
    (handle, addr, temp, password)
}

fn bench_handshake_paths(c: &mut Criterion) {
    // Best-effort tracing init for diagnosing setup; ignore the
    // "already initialised" error when re-run.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .try_init();

    // Multi-thread runtime: TLS + SCRAM use background tasks; the bench
    // body itself is awaited on the runtime's `block_on`.
    // worker_threads matches `tests/duplex_e2e.rs::resume_then_concurrent`
    // which exercises the same connect+resume pattern; smaller pools
    // race the in-process server's connection_acceptor against the
    // bench-side `Client::resume` await and produce an "early eof" on
    // the first iteration.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("rt");

    // Spawn the live server ONCE. Both bench functions reuse it — fresh
    // TCP connection per iteration is what we measure, not the launcher
    // boot cost.
    let (handle, addr, _temp, password) = rt.block_on(setup_live_server());

    // Capture a resumption ticket + pin for the resume-path bench. Doing
    // this in setup means the loop only pays the resume cost, not the
    // full SCRAM cost.
    let (_ticket, pinned_hash) = rt.block_on(async {
        let client = Client::connect(ConnectOptions {
            addr,
            server_name: "localhost".to_string(),
            username: "admin".to_string(),
            password: Zeroizing::new(password.clone()),
            accept_new_host: true,
            trusted_pin: None,
        })
        .await
        .expect("setup connect");
        // Ensure the session is fully registered server-side before
        // closing — mirror the pattern in `tests/duplex_e2e.rs::
        // resume_then_concurrent` which always pings once before
        // closing & resuming.
        client.ping().await.expect("setup ping");
        let ticket = client
            .resumption_ticket()
            .expect("server issues ticket")
            .to_vec();
        let pin = client.server_pub_key_pin();
        client.close().await;
        (ticket, pin)
    });

    let mut g = c.benchmark_group("handshake_paths");
    g.throughput(Throughput::Elements(1));

    // -- full_scram_connect: Argon2id-bound; small sample for sane wall-clock.
    g.sample_size(bu::sample_size(10));
    g.bench_function("full_scram_connect", |b| {
        b.iter(|| {
            rt.block_on(async {
                let client = Client::connect(ConnectOptions {
                    addr,
                    server_name: "localhost".to_string(),
                    username: "admin".to_string(),
                    password: Zeroizing::new(password.clone()),
                    accept_new_host: true,
                    trusted_pin: None,
                })
                .await
                .expect("connect");
                black_box(&client);
                client.close().await;
            });
        });
    });

    // -- resume_fast_path: skip Argon2id; tighter measurement.
    //
    // Tickets are single-use AND the server's resume-cache evicts old
    // entries under churn — a simple rotation chain (use ticket N,
    // capture N+1, repeat) fails after K iterations when the chain
    // accumulates state the server has decided to drop. Cleanest fix:
    // `iter_batched` — setup phase grabs a fresh ticket via a full
    // SCRAM connect for EACH measured iteration. The setup cost is
    // excluded from the measurement, the resume call is what's timed.
    let password_arc = std::sync::Arc::new(password.clone());
    g.sample_size(bu::sample_size(20));
    g.bench_function("resume_fast_path", |b| {
        let pwd = password_arc.clone();
        b.iter_batched(
            || {
                // Fresh ticket per measured iteration. NOT timed.
                rt.block_on(async {
                    let c = Client::connect(ConnectOptions {
                        addr,
                        server_name: "localhost".to_string(),
                        username: "admin".to_string(),
                        password: Zeroizing::new((*pwd).clone()),
                        accept_new_host: true,
                        trusted_pin: None,
                    })
                    .await
                    .expect("setup connect");
                    c.ping().await.expect("setup ping");
                    let t = c
                        .resumption_ticket()
                        .expect("server issues ticket")
                        .to_vec();
                    c.close().await;
                    t
                })
            },
            |ticket| {
                rt.block_on(async {
                    let client = Client::resume(ResumeOptions {
                        addr,
                        server_name: "localhost".to_string(),
                        ticket,
                        pinned_hash,
                    })
                    .await
                    .expect("resume");
                    black_box(&client);
                    client.close().await;
                });
            },
            criterion::BatchSize::PerIteration,
        );
    });

    g.finish();

    // Tear the server down explicitly so the data dir is released before
    // the `TempDir` drop tries to remove it.
    rt.block_on(handle.shutdown());
}

criterion_group!(benches, bench_interactive_tx, bench_handshake_paths);
criterion_main!(benches);
