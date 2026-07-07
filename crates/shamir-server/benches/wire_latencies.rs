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
//! Three workloads:
//!   * `begin_only` — open a tx and immediately roll it back.
//!     Bounded cost of `TxBegin` dispatch + registry insert.
//!   * `begin_execute_commit_minimal` — begin + one trivial insert + commit.
//!     Minimum non-trivial tx lifecycle.
//!   * `begin_execute_n_commit` — same shape but 10 inserts in the
//!     execute step. Shows how lifecycle overhead amortizes over work.
//!
//! ## Group 2 — `handshake_paths`
//!
//! Two workloads that drive a fresh in-process live server via the
//! `tests/common/`-equivalent helper mirrored to `benches/common.rs`
//! (Cargo `[[bench]]` targets cannot reach into `tests/`):
//!
//!   * `full_scram_connect` — `Client::connect()` against a freshly
//!     spawned server. Dominated by Argon2id (memory_kb=19456, t=2,
//!     p=1 — spec floor).
//!   * `resume_fast_path` — a fresh SCRAM connect captures a resumption
//!     ticket + pin per iteration (untimed setup via `bench_batched_async`),
//!     then `Client::resume()` is timed. Tickets are single-use and the
//!     server's resume-cache evicts old entries under churn, so a fresh
//!     ticket per measured iteration is required (mirrors the original
//!     `iter_batched` design).

use std::hint::black_box;
use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

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
use shamir_query_builder::write::upsert;

use shamir_server::db_handler::{DbRequest, DbResponse, ShamirDbHandler};

use shamir_client::{Client, ConnectOptions, ResumeOptions};
use tempfile::TempDir;
use zeroize::Zeroizing;

#[path = "common.rs"]
mod common;

#[global_allocator]
static GLOBAL: sefer_alloc::SeferAlloc = sefer_alloc::SeferAlloc::new();

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

async fn build_handler() -> ShamirDbHandler {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    // `create_db`/`add_repo` (System-owned) persist ResourceMeta::owned_enforced
    // (owner-only 0o700) rather than the old open 0o777 default. `fixture_session()`
    // below is a regular ("alice") session, not a superuser, so it resolves to
    // Actor::User(principal_id("alice")) and needs ownership to pass the gate —
    // stamp the bench db/repo/table with that same actor via the `_as` variants.
    let bench_user = Actor::User(principal_id("alice"));
    shamir.create_db_as("app", bench_user.clone()).await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, bench_user)
        .await
        .expect("add repo");
    ShamirDbHandler::new(Arc::new(shamir))
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
/// builder — no hand-assembled query shape.
fn tx_execute_bytes(tx_handle: u64, n: usize) -> Vec<u8> {
    let mut b = Batch::new();
    b.id("w");
    b.return_only(std::iter::empty::<String>());
    for i in 0..n {
        let id = format!("k{i}");
        b.upsert(
            format!("s{i}"),
            upsert("items")
                .key(mpack!({ "id": @(QueryValue::from(id.as_str())) }))
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

fn setup_block_on<T>(fut: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(fut)
}

// ---------------------------------------------------------------------------
// Group 2 setup: live server + SCRAM connect helpers
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

async fn connect_client(addr: std::net::SocketAddr, password: &[u8]) -> Client {
    Client::connect(ConnectOptions {
        addr,
        server_name: "localhost".to_string(),
        username: "admin".to_string(),
        password: Zeroizing::new(password.to_vec()),
        accept_new_host: true,
        trusted_pin: None,
    })
    .await
    .expect("connect")
}

fn main() {
    // Best-effort tracing init for diagnosing setup; ignore the
    // "already initialised" error when re-run.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .try_init();

    let mut h = Harness::new("wire_latencies", env!("CARGO_MANIFEST_DIR"));

    // -----------------------------------------------------------------
    // Group 1: interactive_tx_lifecycle
    // -----------------------------------------------------------------
    {
        let handler = Arc::new(setup_block_on(build_handler()));
        let session = Arc::new(fixture_session());

        // -- begin_only: cost of opening a tx, then rolling back so the
        //    registry stays empty. We must drain the handle to avoid an
        //    unbounded leak across thousands of iterations.
        {
            let handler = handler.clone();
            let session = session.clone();
            let begin_bytes = tx_begin_bytes();
            h.bench_async("interactive_tx_lifecycle/begin_only", move || {
                let handler = handler.clone();
                let session = session.clone();
                let begin_bytes = begin_bytes.clone();
                async move {
                    let resp = call(&handler, &session, black_box(&begin_bytes)).await;
                    let tx = expect_tx_handle(resp);
                    // Drop the tx via rollback — cheap and keeps the
                    // registry from growing. The bench targets BEGIN
                    // cost; the rollback adds a constant offset.
                    let rb = tx_rollback_bytes(tx);
                    let r = call(&handler, &session, &rb).await;
                    black_box(r);
                }
            });
        }

        // -- begin_execute_commit_minimal: 1 upsert.
        {
            let handler = handler.clone();
            let session = session.clone();
            let begin_bytes = tx_begin_bytes();
            h.bench_async(
                "interactive_tx_lifecycle/begin_execute_commit_minimal",
                move || {
                    let handler = handler.clone();
                    let session = session.clone();
                    let begin_bytes = begin_bytes.clone();
                    async move {
                        let tx = expect_tx_handle(
                            call(&handler, &session, black_box(&begin_bytes)).await,
                        );
                        let exec = tx_execute_bytes(tx, 1);
                        let r1 = call(&handler, &session, &exec).await;
                        black_box(&r1);
                        let c1 = tx_commit_bytes(tx);
                        let r2 = call(&handler, &session, &c1).await;
                        black_box(r2);
                    }
                },
            );
        }

        // -- begin_execute_n_commit: 10 upserts in a single execute.
        {
            let handler = handler.clone();
            let session = session.clone();
            let begin_bytes = tx_begin_bytes();
            h.bench_async(
                "interactive_tx_lifecycle/begin_execute_n_commit",
                move || {
                    let handler = handler.clone();
                    let session = session.clone();
                    let begin_bytes = begin_bytes.clone();
                    async move {
                        let tx = expect_tx_handle(
                            call(&handler, &session, black_box(&begin_bytes)).await,
                        );
                        let exec = tx_execute_bytes(tx, 10);
                        let r1 = call(&handler, &session, &exec).await;
                        black_box(&r1);
                        let c1 = tx_commit_bytes(tx);
                        let r2 = call(&handler, &session, &c1).await;
                        black_box(r2);
                    }
                },
            );
        }
    }

    // -----------------------------------------------------------------
    // Group 2: handshake_paths — full SCRAM connect vs resumption fast-path
    // -----------------------------------------------------------------
    //
    // `handle`/`_temp` are declared in `main`'s own scope (not a nested
    // block) and torn down only after `h.run()` at the bottom of this
    // function. `Harness::bench_async`/`bench_batched_async` only
    // REGISTER closures here — they don't execute them; execution happens
    // inside `h.run()`. A nested block that shuts the server down before
    // `h.run()` runs tears it down before any registered closure ever
    // connects, so every `full_scram_connect`/`resume_fast_path` call
    // hits a dead listener (`ConnectionRefused`).
    //
    // `setup_live_server()` (and the pinned-hash capture below) run on a
    // dedicated PERSISTENT runtime (`server_rt`), NOT `setup_block_on`'s
    // disposable per-call one. `ServerLauncher::launch()` spawns the
    // accept-loop tasks via `tokio::spawn` onto whichever runtime is
    // current at the call site; `setup_block_on` builds a throwaway
    // current-thread runtime and drops it the instant `block_on` returns,
    // which aborts every task spawned on it — including those accept
    // loops and the `TcpListener` they own. The listener socket dies (and
    // every subsequent connect gets `ConnectionRefused`) the moment
    // `setup_live_server()` returns, well before any registered
    // `full_scram_connect`/`resume_fast_path` closure runs. `server_rt`
    // stays alive until after `h.run()` so the accept loops keep running
    // for the whole bench.
    // Multi-thread, NOT `new_current_thread()`: a current-thread runtime
    // only polls tasks spawned via `tokio::spawn` while a `block_on` call
    // is actively driving it on that one thread — the accept-loop tasks
    // `launch()` spawns would go unpolled (and every client connect would
    // hang forever on the TLS handshake, since the kernel completes the
    // bare TCP 3-way handshake via backlog regardless) the instant this
    // setup's `block_on` call returns, until the next explicit `block_on`.
    // A multi-thread runtime keeps worker threads polling spawned tasks
    // continuously, independent of any particular `block_on` call.
    let server_rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("server_rt");
    let (handle, addr, _temp, password) = server_rt.block_on(setup_live_server());
    let password = Arc::new(password);
    {
        // Capture a resumption ticket + pin for the resume-path bench setup
        // needs the pinned hash. Doing this once means the loop only pays
        // the resume cost, not the full SCRAM cost, on the per-iteration
        // setup path below.
        let pinned_hash = server_rt.block_on(async {
            let client = connect_client(addr, &password).await;
            // Ensure the session is fully registered server-side before
            // closing — mirror the pattern in `tests/duplex_e2e.rs::
            // resume_then_concurrent` which always pings once before
            // closing & resuming.
            client.ping().await.expect("setup ping");
            let pin = client.server_pub_key_pin();
            client.close().await;
            pin
        });

        // -- full_scram_connect: Argon2id-bound.
        {
            let password = password.clone();
            h.bench_async("handshake_paths/full_scram_connect", move || {
                let password = password.clone();
                async move {
                    let client = connect_client(addr, &password).await;
                    black_box(&client);
                    client.close().await;
                }
            });
        }

        // -- resume_fast_path: skip Argon2id.
        //
        // Tickets are single-use AND the server's resume-cache evicts old
        // entries under churn — a simple rotation chain (use ticket N,
        // capture N+1, repeat) fails after K iterations when the chain
        // accumulates state the server has decided to drop. Cleanest fix:
        // `bench_batched_async` — setup phase grabs a fresh ticket via a
        // full SCRAM connect for EACH measured iteration. The setup cost is
        // excluded from the measurement, the resume call is what's timed.
        {
            let password = password.clone();
            h.bench_batched_async(
                "handshake_paths/resume_fast_path",
                move || {
                    let password = password.clone();
                    async move {
                        // Fresh ticket per measured iteration. NOT timed.
                        let c = connect_client(addr, &password).await;
                        c.ping().await.expect("setup ping");
                        let ticket = c
                            .resumption_ticket()
                            .expect("server issues ticket")
                            .to_vec();
                        c.close().await;
                        ticket
                    }
                },
                move |ticket| async move {
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
                },
            );
        }
    }

    h.run();

    // Tear the server down explicitly, after every registered closure has
    // actually run, so the data dir is released before the `TempDir` drop
    // (at the end of `main`) tries to remove it. Run on `server_rt` (not
    // `setup_block_on`'s throwaway runtime) since the accept-loop tasks
    // `shutdown()` cancels/joins were themselves spawned on `server_rt`.
    server_rt.block_on(handle.shutdown());
}
