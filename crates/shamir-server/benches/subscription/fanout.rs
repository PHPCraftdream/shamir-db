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
//! `mpsc::unbounded_channel` — no backpressure, no drop.
//!
//! Setup (DB + N subscribers) is built ONCE per `n` variant outside the
//! timed loop (mirrors the original `iter_custom` design, which set up
//! before the sample loop); each measured call fires K inserts and drains
//! K events per subscriber — `bench_async`.

use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use bench_scale_tool::Harness;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinSet;
use tokio::time::timeout;

use shamir_connect::common::push_envelope::{PushEnvelope, PushKind};
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::{ConnectionServices, PushRejected, PushSink};
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};

use shamir_db::access::{principal64, Actor};
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

// `create_db`/`add_repo` (System-owned) persist ResourceMeta::owned_enforced
// (owner-only 0o700) rather than the old open 0o777 default. `fixture_session()`
// above is a regular ("alice") session, not a superuser, so it resolves to
// Actor::User(principal64([0xAB; 16])) and needs ownership to pass the gate.
async fn make_db_one_repo(db: &str, repo: &str, table: &str) -> Arc<ShamirDb> {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let bench_user = Actor::User(principal64([0xAB; 16]));
    shamir.create_db_as(db, bench_user.clone()).await;
    let cfg = RepoConfig::new(repo, BoxRepoFactory::in_memory()).add_table(TableConfig::new(table));
    shamir.add_repo_as(db, cfg, bench_user).await.unwrap();
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
/// `session` / `conn` are kept alive (the conn owns the push sink Arc
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

/// The simple `thread_id == 42` Eq filter used by the standard fan-out benches.
fn default_filter() -> Filter {
    Filter::Eq {
        field: vec!["thread_id".into()],
        value: FilterValue::Int(42),
    }
}

/// Register N subscriptions on the same table with a caller-provided filter.
async fn setup_subscribers_with_filter(
    handler: &ShamirDbHandler,
    n: usize,
    keys: bool,
    filter: Filter,
) -> (Vec<Subscriber>, JoinSet<()>) {
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
            .filter(filter.clone())
            .build();
        let mut sub_builder = Subscribe::source(src);
        if keys {
            sub_builder = sub_builder.deliver_keys();
        }
        let sub_op = sub_builder.build();
        let mut sb = Batch::new();
        sb.id((i as u64) + 1);
        sb.subscribe("m", sub_op);
        let resp = decode(
            &handler
                .handle(&session, &encode(&execute_built("app", sb.build())), &conn)
                .await
                .unwrap(),
        );
        let sub_id = {
            use shamir_types::types::value::QueryValue;
            match resp {
                DbResponse::Batch { response } => {
                    let v = response
                        .results
                        .get("m")
                        .and_then(|qr| qr.value.as_ref())
                        .expect("subscribe result has no value");
                    match v {
                        QueryValue::Map(m) => match m.get("sub") {
                            Some(QueryValue::Int(id)) => *id as u64,
                            other => panic!("sub field not Int: {:?}", other),
                        },
                        other => panic!("value not Map: {:?}", other),
                    }
                }
                _ => panic!("expected Batch response"),
            }
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

const K: usize = 20; // inserts per measured iteration

/// Fixed setup shared by all iterations of one `n` variant: DB, handler,
/// subscribers, writer session/conn. Wrapped in a `Mutex` because
/// `subs`/`joinset` are mutated per-iteration (drain + abort at the very
/// end of the whole bench, not per iteration) — but the harness only
/// needs shared *read* access across repeated `bench_async` calls, so an
/// interior `tokio::sync::Mutex` mirrors the original single-threaded
/// `iter_custom` closure's exclusive access to `subs`.
struct FanoutFixture {
    handler: ShamirDbHandler,
    subs: tokio::sync::Mutex<Vec<Subscriber>>,
    writer_session: Session,
    writer_conn: ConnectionServices,
    n: usize,
    /// `true` for the `$in`-filter variant, whose inserted `body` values
    /// must cycle `msg-0..msg-9` (`i % 10`) to exercise the `$in` match;
    /// the standard/keys variants use a distinct `body` per insert
    /// (`msg-{i}`) since they filter on `thread_id` alone.
    body_mod_10: bool,
}

async fn build_fanout_fixture(
    n: usize,
    keys: bool,
    filter: Filter,
    body_mod_10: bool,
) -> Arc<FanoutFixture> {
    let shamir = make_db_one_repo("app", "main", "messages").await;
    let handler = ShamirDbHandler::new(shamir);
    let (subs, _joinset) = setup_subscribers_with_filter(&handler, n, keys, filter).await;

    // Writer needs ITS own session/conn (any will do).
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

    Arc::new(FanoutFixture {
        handler,
        subs: tokio::sync::Mutex::new(subs),
        writer_session,
        writer_conn,
        n,
        body_mod_10,
    })
}

async fn run_fanout_iter(fixture: Arc<FanoutFixture>, iter_idx: u64) {
    let n = fixture.n;
    // Fire K inserts (writes happen once; fan-out is per-sub).
    for i in 0..K {
        let mut wb = Batch::new();
        wb.id((iter_idx * 1000) + (i as u64) + 10_000);
        let body = if fixture.body_mod_10 {
            format!("msg-{}", i % 10)
        } else {
            format!("msg-{i}")
        };
        wb.insert(
            "ins",
            insert("messages").row(
                doc! { "_id" => format!("k{}-{}", iter_idx, i) }
                    .set("thread_id", 42_i64)
                    .set("body", body),
            ),
        );
        let _ = fixture
            .handler
            .handle(
                &fixture.writer_session,
                &encode(&execute_built("app", wb.build())),
                &fixture.writer_conn,
            )
            .await
            .unwrap();
    }

    // Drain K events per subscriber. Sum all = N*K.
    let mut subs = fixture.subs.lock().await;
    let mut got_total = 0usize;
    for sub in subs.iter_mut() {
        let got = drain_events_for(&mut sub.rx, sub.sub_id, K, Duration::from_secs(15)).await;
        got_total += got;
        black_box(got);
    }
    assert_eq!(
        got_total,
        n * K,
        "fanout loss: got {got_total}/{} for n={n} K={K}",
        n * K
    );
}

/// `build_fanout_fixture` spawns one `bridge_task` per subscriber (see the
/// module doc: "one `bridge_task` per sub drains the changefeed") via
/// `tokio::spawn` — a background task that must keep running for the
/// fixture's whole lifetime. Building the fixture on a throwaway
/// `new_current_thread()` runtime (as this bench used to, via a
/// `setup_block_on` matching `wire_latencies.rs`'s old helper) aborts every
/// task spawned on it the instant `block_on` returns — every bridge task
/// dies, so every subscriber receives zero events forever after
/// (`fanout loss: got 0/... `). `register_fanout_variant` therefore takes a
/// PERSISTENT multi-thread runtime (built once in `main`, kept alive until
/// after `h.run()`) so the bridge tasks it spawns keep draining the
/// changefeed for the whole bench.
fn register_fanout_variant(
    h: &mut Harness,
    rt: &tokio::runtime::Runtime,
    group: &str,
    n: usize,
    keys: bool,
    filter: Filter,
    body_mod_10: bool,
) {
    let fixture = rt.block_on(build_fanout_fixture(n, keys, filter, body_mod_10));
    let iter_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let id = format!("{group}/n_{n}");
    h.bench_async(&id, move || {
        let fixture = fixture.clone();
        let iter_idx = iter_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        async move {
            run_fanout_iter(fixture, iter_idx).await;
        }
    });
}

fn main() {
    let mut h = Harness::new("subscription_fanout", env!("CARGO_MANIFEST_DIR"));

    // See `register_fanout_variant`'s doc comment: must be multi-thread and
    // must outlive `h.run()` so every subscriber's `bridge_task` keeps
    // running for the whole bench.
    let server_rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("server_rt");

    // N = subscriber count is a genuine structural axis (fan-out scaling:
    // does per-event delivery cost grow linearly or worse with N?).
    // Default = smallest tier only so each call stays cheap; set
    // BENCH_SUBSCRIPTION_FANOUT_SCALING=1 to run the full ladder.
    let wide = std::env::var("BENCH_SUBSCRIPTION_FANOUT_SCALING")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);
    let ns: &[usize] = if wide {
        &[1usize, 10, 50, 100]
    } else {
        &[1usize]
    };

    for &n in ns {
        register_fanout_variant(
            &mut h,
            &server_rt,
            "subscription_fanout",
            n,
            false,
            default_filter(),
            false,
        );
    }

    for &n in ns {
        register_fanout_variant(
            &mut h,
            &server_rt,
            "subscription_fanout_keys",
            n,
            true,
            default_filter(),
            false,
        );
    }

    // Keys-mode fan-out. Identical scaffolding, but every subscription uses
    // `DeliverMode::Keys`. This is the path where the `value_qv` de-intern
    // deferral wins unconditionally — Keys never reads `value_qv`, so the
    // decode is pure waste on every event, hit or miss, multiplied by N
    // subscribers.
    //
    // Fan-out with an `$in` filter containing 10 string literals. This is
    // the supported-filter path where compile caching has the most impact
    // among allowed subscription operators: `compile_filter` builds a
    // `TSet<QueryValue>` (hashing all 10 values) and clones each String —
    // work that is currently repeated per event × per subscriber.
    //
    // (Note: `Like`/`Regex` are the biggest theoretical win, but they are
    // currently blocked by `find_unsupported_subscription_filter`. The `$in`
    // path exercises the `TSet` build + `FilterValue::clone` costs that are
    // eliminated by compile-once caching.)
    for &n in ns {
        let filter = Filter::In {
            field: vec!["body".into()],
            values: (0..10)
                .map(|i| FilterValue::String(format!("msg-{i}")))
                .collect(),
        };
        register_fanout_variant(
            &mut h,
            &server_rt,
            "subscription_fanout_in",
            n,
            false,
            filter,
            true,
        );
    }

    h.run();
}
