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

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::common::time::UnixNanos;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::Session;
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

fn bench(c: &mut Criterion) {
    let (handler, session) = build_handler();
    let ping_bytes = rmp_serde::to_vec_named(&DbRequest::Ping).unwrap();

    let mut g = c.benchmark_group("db_handler");
    g.throughput(Throughput::Elements(1));
    g.bench_function("ping_inprocess", |b| {
        b.iter(|| {
            let resp = handler
                .handle(black_box(&session), black_box(&ping_bytes))
                .unwrap();
            black_box(resp);
        });
    });
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
