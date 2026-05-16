//! In-process RPS bench for `ShamirDbHandler::handle`.
//!
//! Spins up an in-memory ShamirDb, builds a fixture Session, then loops
//! `handler.handle(&session, &req_bytes)` in a tight loop. Measures the
//! cost of every layer the request envelope flows through once it's been
//! decoded into bytes — msgpack decode, dispatch match, response encode.
//!
//! This is NOT a network-level bench (no TLS, no TCP, no Argon2). It's the
//! in-process upper bound — anything the network layer adds on top is
//! transport overhead, not server-side processing.

use std::sync::Arc;

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use serde_json::json;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::common::time::UnixNanos;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::Session;
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::query::batch::BatchRequest;
use shamir_db::ShamirDb;
use shamir_server::db_handler::{DbRequest, ShamirDbHandler};

fn fixture_session() -> Session {
    Session::new(
        [0x01u8; 16],
        "alice".into(),
        // Bench bypasses the permissions gate by giving the session every
        // role we use below. The dispatch path is what we want to measure.
        shamir_connect::server::session::SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0x77u8; 32],
        UnixNanos::now().as_u64(),
    )
}

fn build_handler() -> (ShamirDbHandler, Session) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let db = rt.block_on(async { ShamirDb::init_memory().await.unwrap() });
    let handler = ShamirDbHandler::new(Arc::new(db));
    let session = fixture_session();
    (handler, session)
}

/// Spin up a handler with a `prod` DB / `main` repo / `users` table
/// already created and `count` records inserted. Returns
/// `(handler, session, runtime)` — the runtime owns its current_thread
/// loop so callers can issue more async setup if needed.
fn build_loaded_handler(count: usize) -> (ShamirDbHandler, Session, tokio::runtime::Runtime) {
    // ShamirDbHandler::handle uses `tokio::task::block_in_place` for the
    // execute path — requires a multi-thread runtime. Single-worker is
    // fine; we just need the right flavour.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let (handler, session) = rt.block_on(async {
        let shamir = ShamirDb::init_memory().await.unwrap();
        shamir.create_db("prod").await;
        let cfg = RepoConfig::new("main", BoxRepoFactory::in_memory())
            .add_table(TableConfig::new("users"));
        shamir.add_repo("prod", cfg).await.unwrap();

        let handler = ShamirDbHandler::new(Arc::new(shamir));
        let session = fixture_session();

        // Seed `count` records via one batch of Set ops. Done through
        // the handler so we know the table really has the data the
        // bench query will read.
        let cities = ["NYC", "LA", "Boston", "Seattle"];
        let mut queries = serde_json::Map::new();
        for i in 0..count {
            let city = cities[i % 4];
            queries.insert(
                format!("s{}", i),
                json!({
                    "set": "users",
                    "key": { "id": format!("user-{}", i) },
                    "value": {
                        "id": format!("user-{}", i),
                        "name": format!("Name {}", i),
                        "age": (i % 100) as i64,
                        "city": city,
                        "active": i % 2 == 0,
                    }
                }),
            );
        }
        let seed_body = json!({ "id": "seed", "queries": queries, "return_all": false });
        let batch: BatchRequest = serde_json::from_value(seed_body).unwrap();
        let seed_req = DbRequest::Execute {
            query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
            db: "prod".into(),
            batch,
        };
        let bytes = rmp_serde::to_vec_named(&seed_req).unwrap();
        handler.handle(&session, &bytes).unwrap();

        (handler, session)
    });
    (handler, session, rt)
}

/// Encode a `DbRequest::Execute` for `prod` from a JSON batch body. The
/// resulting bytes are what the bench `handle()` call decodes on every
/// iteration — same path as production wire traffic.
fn encode_execute(body: serde_json::Value) -> Vec<u8> {
    let batch: BatchRequest = serde_json::from_value(body).unwrap();
    let req = DbRequest::Execute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: "prod".into(),
        batch,
    };
    rmp_serde::to_vec_named(&req).unwrap()
}

fn bench(c: &mut Criterion) {
    let mut g = c.benchmark_group("db_handler");
    g.throughput(Throughput::Elements(1));

    // -- ping: dispatch upper bound (decode + match + encode only).
    {
        let (handler, session) = build_handler();
        let ping_bytes = rmp_serde::to_vec_named(&DbRequest::Ping).unwrap();
        g.bench_function("ping_inprocess", |b| {
            b.iter(|| {
                let resp = handler
                    .handle(black_box(&session), black_box(&ping_bytes))
                    .unwrap();
                black_box(resp);
            });
        });
    }

    // -- realistic Execute path: 100 records preloaded, filter + select +
    //    order_by + pagination. Same shape as a typical "list page" query.
    {
        let (handler, session, rt) = build_loaded_handler(100);
        let read_bytes = encode_execute(json!({
            "id": "rd",
            "queries": {
                "page": {
                    "from": "users",
                    "where": { "op": "gte", "field": ["age"], "value": 30 },
                    "select": {
                        "items": [
                            { "type": "field", "path": ["name"] },
                            { "type": "field", "path": ["age"] },
                            { "type": "field", "path": ["city"] },
                        ]
                    },
                    "order_by": { "items": [{ "field": ["age"], "direction": "desc" }] },
                    "pagination": { "mode": "LimitOffset", "limit": 20, "offset": 0 }
                }
            }
        }));
        g.bench_function("execute_read_filter_sort_limit_100records", |b| {
            // `block_in_place` inside `run_blocking` (db_handler.rs:549) wants
            // a tokio task context — drive iter through the runtime's
            // `block_on` so each invocation runs under that context.
            b.iter(|| {
                rt.block_on(async {
                    let resp = handler
                        .handle(black_box(&session), black_box(&read_bytes))
                        .unwrap();
                    black_box(resp);
                });
            });
        });
    }

    // -- simple read: full scan, no filter, no order. Tests the encode-only
    //    cost of returning the full row set.
    {
        let (handler, session, rt) = build_loaded_handler(100);
        let read_bytes = encode_execute(json!({
            "id": "rd2",
            "queries": {
                "all": { "from": "users" }
            }
        }));
        g.bench_function("execute_full_scan_100records", |b| {
            b.iter(|| {
                rt.block_on(async {
                    let resp = handler
                        .handle(black_box(&session), black_box(&read_bytes))
                        .unwrap();
                    black_box(resp);
                });
            });
        });
    }

    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
