//! End-to-end throughput bench for Live Subscriptions v1.1.
//!
//! Wires the production stack — `ShamirDbHandler`, `SubscriptionRegistry`,
//! `bridge_task`, the changefeed broadcast — and drives an insert → push
//! frame delivery loop. The bench measures the WHOLE per-event path:
//! changefeed emit → broadcast deliver → de-intern → filter → payload →
//! `PushSink::try_push`. That's the cost a real client sees on every
//! delivered change.

use std::sync::Arc;
use std::time::Duration;

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
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

/// Drain push frames from `rx` until `target` Event frames for `sub_id`
/// arrive (drops Ready/Gap/Closed/etc). Returns the number of events drained.
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

fn bench_bridge_throughput_single_sub(c: &mut Criterion) {
    let mut g = c.benchmark_group("subscription_bridge_throughput");
    const N: usize = 100;
    g.throughput(Throughput::Elements(N as u64));
    g.sample_size(bu::sample_size(10));

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();

    g.bench_function("single_sub_filtered_inserts_100", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async move {
                let mut total = std::time::Duration::ZERO;
                for _ in 0..iters {
                    // Fresh DB + registry per iter so subscription IDs don't pile up.
                    let shamir = make_db_one_repo("app", "main", "messages").await;
                    let handler = ShamirDbHandler::new(shamir);
                    let s = fixture_session();
                    let (push, mut rx) = capture();
                    let registry = Arc::new(SubscriptionRegistry::new());
                    let conn = conn_with(push, registry.clone());

                    // Subscribe with a value filter that matches every insert.
                    let src = SourceBuilder::table(TableRef::with_repo("main", "messages"))
                        .filter(Filter::Eq {
                            field: vec!["thread_id".into()],
                            value: FilterValue::Int(42),
                        })
                        .build();
                    let sub_op = Subscribe::source(src).build();
                    let mut sb = Batch::new();
                    sb.id(1);
                    sb.subscribe("m", sub_op);
                    let resp = decode(
                        &handler
                            .handle(&s, &encode(&execute_built("app", sb.build())), &conn)
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

                    // Let the bridge attach to the changefeed.
                    tokio::time::sleep(Duration::from_millis(80)).await;

                    let start = std::time::Instant::now();
                    // Fire N inserts.
                    for i in 0..N {
                        let mut wb = Batch::new();
                        wb.id((i as u64) + 100);
                        wb.insert(
                            "ins",
                            insert("messages").row(
                                doc! { "_id" => format!("k{i}") }
                                    .set("thread_id", 42_i64)
                                    .set("body", format!("msg-{i}")),
                            ),
                        );
                        let _ = handler
                            .handle(&s, &encode(&execute_built("app", wb.build())), &conn)
                            .await
                            .unwrap();
                    }
                    let got = drain_events_for(&mut rx, sub_id, N, Duration::from_secs(10)).await;
                    let elapsed = start.elapsed();
                    assert_eq!(got, N, "did not receive all events: got {got}/{N}");
                    total += elapsed;
                    black_box(sub_id);
                }
                total
            })
        });
    });

    g.finish();
}

criterion_group!(benches, bench_bridge_throughput_single_sub);
criterion_main!(benches);
