//! Benchmark: lock-step (max_in_flight=1) vs duplex-32 (max_in_flight=32).
//!
//! Uses `request_loop` over `tokio::io::duplex` — same transport path as the
//! `duplex_loop` integration tests, but driven by criterion for stable timing.
//!
//! No TLS, no Argon2id, no TCP — those are transport-layer overheads that sit
//! **on top of** the in-process dispatch we want to measure here.
//!
//! ## Benchmark design
//!
//! Each iteration opens a fresh `tokio::io::duplex` pair, spawns
//! `request_loop`, sends N pre-built request frames, and drains N responses.
//!
//! The handler (`DelayHandler`) sleeps 1 ms per request to simulate realistic
//! IO-bound latency.  With `max_in_flight=32` all N requests overlap; with
//! `max_in_flight=1` they serialise, taking N × 1 ms.  The speedup is
//! therefore bounded by `min(N, 32)`.
//!
//! `ConnectionContext` is constructed once per benchmark group (outside the
//! timed `b.iter` region) to avoid measuring redb setup overhead.

use std::sync::Arc;
use std::time::Duration;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use tokio::io::AsyncWriteExt;

use shamir_connect::common::envelope::{RequestEnvelope, ResponseEnvelope};
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::dispatch::{HandlerFuture, RequestHandler};
use shamir_connect::server::session::{Session, SessionPermissions, SessionStore};

use shamir_server::connection::{request_loop, ConnectionContext};
use shamir_server::framer::{Framer, TcpFramer};

use shamir_transport_tcp::framing::{read_frame_into, write_frame_into, MAX_FRAME_SIZE_DEFAULT};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

// ---------------------------------------------------------------------------
// Stub handler — 1 ms latency per request (simulates IO-bound work).
// ---------------------------------------------------------------------------

struct DelayHandler;

impl RequestHandler for DelayHandler {
    fn handle<'a>(&'a self, _session: &'a Session, req: &'a [u8]) -> HandlerFuture<'a> {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(1)).await;
            Ok(req.to_vec())
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const SESSION_ID: [u8; 32] = [0xABu8; 32];

/// Build a minimal `ConnectionContext` with `max_in_flight`.
/// The `ConnectionContext` is re-used across iterations — only the
/// `tokio::io::duplex` pair and the `request_loop` task are rebuilt per
/// iteration.
fn make_ctx(max_in_flight: usize) -> Arc<ConnectionContext> {
    // ConnectionContext::new returns Arc<Self> directly.
    use shamir_connect::common::crypto::Ed25519Keypair;
    use shamir_connect::common::kdf_params::KdfParams;
    use shamir_connect::server::argon2_semaphore::Argon2Semaphore;
    use shamir_connect::server::audit_chain::{
        AuditAppender, AuditChain, AuditChainWriter, AuditEntry,
    };
    use shamir_connect::server::config::ServerSecrets;
    use shamir_connect::server::lockout::InMemoryLockoutStore;
    use shamir_connect::server::rate_limit::InMemoryRateLimiter;
    use shamir_connect::server::resume::{InMemoryConsumedCounters, ResumeConfig};
    use shamir_connect::server::rotation::ServerIdentityState;

    let kdf = KdfParams {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    };
    let seed = [0x11u8; 32];
    let identity = ServerIdentityState::from_material(&seed, None, None, 1);
    let keypair = Ed25519Keypair::from_seed(&seed);
    let secrets = Arc::new(ServerSecrets {
        server_secret: [0x22u8; 32],
        lockout_secret: [0x33u8; 32],
    });
    let session_store = Arc::new(SessionStore::new());
    let session = Session::new(
        [0u8; 16],
        "bench".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        1_000_000,
    );
    session_store.insert(SESSION_ID, session);

    let temp = tempfile::TempDir::new().unwrap();
    let temp_path = temp.path().join("u.redb");
    let user_dir = shamir_server::user_directory::RedbUserDirectory::open(&temp_path)
        .expect("open user dir for bench");
    // Leak the temp dir so the redb file outlives this function.
    Box::leak(Box::new(temp));

    let chain = AuditChain::new([0u8; 32]);
    struct NoopAppender;
    impl AuditAppender for NoopAppender {
        fn append_entry(&self, _entry: &AuditEntry) {}
        fn checkpoint(&self, _next_seq: u64, _prev_hmac: &[u8; 32]) {}
    }
    let appender = Arc::new(NoopAppender) as Arc<dyn AuditAppender>;
    let audit = Arc::new(AuditChainWriter::new(chain, appender));
    let resume = Arc::new(ResumeConfig::new([0x44u8; 32], None, false, false));
    let counters = Arc::new(InMemoryConsumedCounters::new());

    ConnectionContext::new(
        Arc::new(identity),
        keypair,
        secrets,
        kdf,
        session_store,
        Arc::new(user_dir),
        Arc::new(InMemoryLockoutStore::new()),
        Arc::new(InMemoryRateLimiter::new(0)),
        Arc::new(Argon2Semaphore::with_capacity(4)),
        audit,
        resume,
        counters,
        Arc::new(DelayHandler),
        BindingMode::TlsExporter,
        TransportKind::Tcp,
        None,
        Duration::from_secs(5),
        max_in_flight,
    )
}

/// Pre-build length-framed request bytes for rid 1..=n.
fn build_frames(n: u32) -> Vec<Vec<u8>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    (1..=n)
        .map(|rid| {
            let env = RequestEnvelope {
                session_id: SESSION_ID.to_vec(),
                request_id: Some(rid),
                req: b"ping".to_vec(),
            };
            let msgpack = env.to_msgpack().unwrap();
            let mut frame = Vec::new();
            rt.block_on(async {
                let mut scratch = Vec::new();
                write_frame_into(&mut frame, &msgpack, &mut scratch)
                    .await
                    .unwrap();
            });
            frame
        })
        .collect()
}

/// Timed region: open a duplex pair, spawn `request_loop`, send N frames,
/// drain N responses, then tear down.
fn run_round(rt: &tokio::runtime::Runtime, ctx: Arc<ConnectionContext>, frames: &[Vec<u8>]) {
    let n = frames.len();
    let frames_owned: Vec<Vec<u8>> = frames.to_vec();

    rt.block_on(async move {
        // Generous buffer: 4 MiB prevents send-side blocking.
        let (server_io, client_io) = tokio::io::duplex(4 * 1024 * 1024);
        let (server_r, server_w) = TcpFramer::new(server_io).split();

        let loop_task = tokio::spawn(request_loop(ctx, server_r, server_w, SESSION_ID));

        let (mut cr, mut cw) = tokio::io::split(client_io);

        // Sender: fire all N frames without waiting for responses.
        let send_task = tokio::spawn(async move {
            for frame in frames_owned {
                cw.write_all(&frame).await.unwrap();
            }
            cw
        });

        // Receiver: drain exactly N responses.
        let mut buf = Vec::new();
        for _ in 0..n {
            buf.clear();
            read_frame_into(&mut cr, MAX_FRAME_SIZE_DEFAULT, &mut buf)
                .await
                .unwrap();
            let resp = ResponseEnvelope::from_msgpack(&buf).unwrap();
            black_box(resp);
        }

        // Teardown: drop both client halves to signal EOF to the server.
        let cw = send_task.await.unwrap();
        drop(cw);
        drop(cr);
        let _ = loop_task.await;
    });
}

// ---------------------------------------------------------------------------
// Benchmark groups
// ---------------------------------------------------------------------------

fn bench_duplex_vs_lockstep(c: &mut Criterion) {
    // Multi-thread runtime: request_loop uses spawn internally.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();

    let mut group = c.benchmark_group("duplex_throughput");
    // Extend target time: with 1 ms/request × 100 requests × cap=1, each
    // lockstep iteration takes ~100 ms.
    group.measurement_time(Duration::from_secs(20));
    group.sample_size(20);

    for &n in &[10u32, 32u32] {
        group.throughput(Throughput::Elements(n as u64));
        let frames = build_frames(n);

        // --- lock-step: max_in_flight = 1 ---
        let ctx_lockstep = make_ctx(1);
        group.bench_with_input(BenchmarkId::new("lockstep_cap1", n), &n, |b, _| {
            b.iter(|| run_round(&rt, Arc::clone(&ctx_lockstep), &frames));
        });

        // --- duplex: max_in_flight = 32 ---
        let ctx_duplex = make_ctx(32);
        group.bench_with_input(BenchmarkId::new("duplex_cap32", n), &n, |b, _| {
            b.iter(|| run_round(&rt, Arc::clone(&ctx_duplex), &frames));
        });
    }

    group.finish();
}

criterion_group!(benches, bench_duplex_vs_lockstep);
criterion_main!(benches);
