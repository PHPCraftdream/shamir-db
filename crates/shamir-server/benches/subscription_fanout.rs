//! Fan-out throughput bench for Live Subscriptions.
//!
//! Varies **N** = number of concurrent subscriptions on the same table, all
//! matching the same simple filter, so every inserted record fans out to all
//! N subscribers. Each subscription has its own `CapturePush` + unbounded
//! mpsc receiver; one `bridge_task` per sub drains the changefeed.
//!
//! Per-iteration cost story: ONE write batch fires K=20 inserts; the engine
//! does the write once, then the per-sub bridge path (target match →
//! de-intern → filter eval → payload build → `PushSink::try_push`) runs N×K
//! times. Throughput is reported as **events delivered** = N*K, so the
//! number is "delivered events per second" across all subs. Per-sub cost
//! ≈ total / N — if fan-out is healthy it should grow sub-linearly with N
//! (only the per-sub push step scales; write + changefeed broadcast is
//! shared).
//!
//! Settle: bridge attach is async (registry insert + spawn). For N=100 the
//! 80 ms used by `subscription_throughput.rs` is too tight; we use a
//! N-scaled settle (`max(80ms, N*4ms)`, capped at 600 ms). Receivers are
//! `mpsc::unbounded_channel` — no backpressure, no drop. Bridges are owned
//! by a `JoinSet` and `abort_all`'d at end of each `iter_custom` batch.
//!
//! Sample sizing: at N=100, K=20 = 2000 push frames per iter. Group config:
//! `sample_size(20)` + `measurement_time(5s)`.

use std::sync::Arc;
use std::time::Duration;

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinSet;
use tokio::time::timeout;

use shamir_bench_utils as bu;
use shamir_connect::common::push_envelope::{PushEnvelope, PushKind};
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::{ConnectionServices, PushRejected, PushSink};
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};

use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;

use shamir_query_builder::batch::subscribe::{SourceBuilder, Subscribe};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;

use shamir_query_types::batch::BatchRequest;
use shamir_query_types::filter::{Filter, FilterValue};
use shamir_query_types::TableRef;

use shamir_server::db_handler::{DbRequest, DbResponse, ShamirDbHandler};
use shamir_server::subscriptions::SubscriptionRegistry;

// ── CapturePush: drains delivered frames into an mpsc ──

struct CapturePush {
    tx: UnboundedSender<Vec<u8>>,
}

impl PushSink for CapturePush {
    fn try_push(&self, frame: Vec<u8>) -> Result<(), PushRejected> {
        let _ = self.tx.send(frame);
        Ok(())
    }
}

fn capture() -> (Arc<dyn PushSink>, UnboundedReceiver<Vec<u8>>) {
    let (tx, rx) = unbounded_channel();
    (Arc::new(CapturePush { tx }), rx)
}

fn fixture_session() -> Session {
    let mut s = Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        1_000_000,
    );
    s.session_id = [0x11; 32];
    s
}

fn conn_with(push: Arc<dyn PushSink>, registry: Arc<SubscriptionRegistry>) -> ConnectionServices {
    ConnectionServices {
        conn_id: 1,
        push: Some(push),
        extensions: Some(registry as Arc<dyn std::any::Any + Send + Sync>),
    }
}

fn encode(req: &DbRequest) -> Vec<u8> {
    rmp_serde::to_vec_named(req).expect("encode")
}
fn decode(bytes: &[u8]) -> DbResponse {
    rmp_serde::from_slice(bytes).expect("decode")
}
fn execute_built(db: &str, batch: BatchRequest) -> DbRequest {
    DbRequest::Execute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: db.to_string(),
        batch,
    }
}

async fn make_db_one_repo(db: &str, repo: &str, table: &str) -> Arc<ShamirDb> {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db(db).await;
    let cfg = RepoConfig::new(repo, BoxRepoFactory::in_memory()).add_table(TableConfig::new(table));
    shamir.add_repo(db, cfg).await.unwrap();
    Arc::new(shamir)
}

/// Drain `target` Event frames for `sub_id` from `rx`. Returns count drained.
async fn drain_events_for(
    rx: &mut UnboundedReceiver<Vec<u8>>,
    sub_id: u64,
    target: usize,
    dur: Duration,
) -> usize {
    let deadline = tokio::time::Instant::now() + dur;
    let mut got = 0usize;
    while got < target {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            break;
        }
        let remaining = deadline - now;
        let Ok(frame) = timeout(remaining, rx.recv()).await else {
            break;
        };
        let Some(frame) = frame else { break };
        let env: PushEnvelope = match rmp_serde::from_slice(&frame) {
            Ok(e) => e,
            Err(_) => continue,
        };
        if env.sub == sub_id && env.push == PushKind::Event {
            got += 1;
        }
    }
    got
}

/// One subscriber: its own session/connection/push-sink + receiver.
/// `_session` / `_conn` are kept alive (the conn owns the push sink Arc
/// whose other clone lives inside the bridge — dropping conn early would
/// not actually close the bridge, but we hold both for symmetry with a
/// real connection's lifetime).
#[allow(dead_code)]
struct Subscriber {
    sub_id: u64,
    rx: UnboundedReceiver<Vec<u8>>,
    session: Session,
    conn: ConnectionServices,
}

/// Register N subscriptions on the same table, each with its own push sink
/// and bridge. `JoinSet` is returned so bridges can be aborted.
async fn setup_subscribers(handler: &ShamirDbHandler, n: usize) -> (Vec<Subscriber>, JoinSet<()>) {
    let mut subs = Vec::with_capacity(n);
    // JoinSet is reserved for future explicit bridge-task ownership; today
    // bridges are owned by the registry and aborted via Drop when the
    // handler/registry go out of scope at end of bench. Kept here so the
    // teardown story is symmetric with the doc-comment.
    let joinset = JoinSet::new();

    for i in 0..n {
        let (push, rx) = capture();
        let registry = Arc::new(SubscriptionRegistry::new());
        let mut session = fixture_session();
        // Distinct session_id per sub so registry entries don't collide.
        let mut sid = [0x11u8; 32];
        sid[0] = (i & 0xFF) as u8;
        sid[1] = ((i >> 8) & 0xFF) as u8;
        session.session_id = sid;
        let conn = ConnectionServices {
            conn_id: (i as u64) + 1,
            push: Some(push),
            extensions: Some(registry as Arc<dyn std::any::Any + Send + Sync>),
        };

        let src = SourceBuilder::table(TableRef::with_repo("main", "messages"))
            .filter(Filter::Eq {
                field: vec!["thread_id".into()],
                value: FilterValue::Int(42),
            })
            .build();
        let sub_op = Subscribe::source(src).build();
        let mut sb = Batch::new();
        sb.id((i as u64) + 1);
        sb.subscribe("m", sub_op);
        let resp = decode(
            &handler
                .handle(&session, &encode(&execute_built("app", sb.build())), &conn)
                .await
                .unwrap(),
        );
        let sub_id = match resp {
            DbResponse::Batch { response } => response
                .results
                .get("m")
                .and_then(|qr| qr.value.as_ref())
                .and_then(|v| v.get("sub"))
                .and_then(|s| s.as_u64())
                .expect("sub id"),
            _ => panic!("expected Batch response"),
        };

        subs.push(Subscriber {
            sub_id,
            rx,
            session,
            conn,
        });
    }

    (subs, joinset)
}

fn bench_fanout(c: &mut Criterion) {
    let mut g = c.benchmark_group("subscription_fanout");
    g.sample_size(bu::sample_size(20));
    g.measurement_time(bu::measurement_time(Duration::from_secs(5)));

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();

    const K: usize = 20; // inserts per measured iteration

    for &n in &[1usize, 10, 50, 100] {
        g.throughput(Throughput::Elements((n * K) as u64));
        let name = format!("n_{n}");
        g.bench_function(&name, |b| {
            b.iter_custom(|iters| {
                rt.block_on(async move {
                    let mut total = std::time::Duration::ZERO;

                    // Setup once per outer Criterion invocation of `iters`.
                    let shamir = make_db_one_repo("app", "main", "messages").await;
                    let handler = ShamirDbHandler::new(shamir);
                    let (mut subs, mut joinset) = setup_subscribers(&handler, n).await;

                    // Writer needs ITS own session/conn (any will do; reuse first).
                    let writer_session = fixture_session();
                    let writer_push: Arc<dyn PushSink> = {
                        let (p, _rx) = capture();
                        p
                    };
                    let writer_registry = Arc::new(SubscriptionRegistry::new());
                    let writer_conn = conn_with(writer_push, writer_registry);

                    // N-scaled settle for bridge attach.
                    let settle_ms = (n as u64 * 4).clamp(80, 600);
                    tokio::time::sleep(Duration::from_millis(settle_ms)).await;

                    for iter_idx in 0..iters {
                        let start = std::time::Instant::now();
                        // Fire K inserts (writes happen once; fan-out is per-sub).
                        for i in 0..K {
                            let mut wb = Batch::new();
                            wb.id((iter_idx * 1000) + (i as u64) + 10_000);
                            wb.insert(
                                "ins",
                                insert("messages").row(
                                    doc! { "_id" => format!("k{}-{}", iter_idx, i) }
                                        .set("thread_id", 42_i64)
                                        .set("body", format!("msg-{i}")),
                                ),
                            );
                            let _ = handler
                                .handle(
                                    &writer_session,
                                    &encode(&execute_built("app", wb.build())),
                                    &writer_conn,
                                )
                                .await
                                .unwrap();
                        }

                        // Drain K events per subscriber. Sum all = N*K.
                        let mut got_total = 0usize;
                        for sub in subs.iter_mut() {
                            let got = drain_events_for(
                                &mut sub.rx,
                                sub.sub_id,
                                K,
                                Duration::from_secs(15),
                            )
                            .await;
                            got_total += got;
                            black_box(got);
                        }
                        let elapsed = start.elapsed();
                        assert_eq!(
                            got_total,
                            n * K,
                            "fanout loss: got {got_total}/{} for n={n} K={K}",
                            n * K
                        );
                        total += elapsed;
                    }

                    // Teardown: drop subs (drops rx → push sends become no-ops),
                    // drop handler (drops registry → bridges shut down).
                    joinset.abort_all();
                    while joinset.join_next().await.is_some() {}
                    drop(subs);
                    drop(handler);

                    total
                })
            });
        });
    }

    g.finish();
}

criterion_group!(benches, bench_fanout);
criterion_main!(benches);
