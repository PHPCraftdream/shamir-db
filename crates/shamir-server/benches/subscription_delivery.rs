//! Live Subscriptions — delivery-mode benches (Д4 snapshot, Д5 reactive).
//!
//! Covers paths the sibling `subscription_throughput.rs` bench skips:
//!
//! * **Snapshot** (`initial: true`) — on subscribe, the bridge scans the
//!   target table, emits one `Event` per row, then a terminal `Ready`
//!   frame. Worst-case latency a client sees before live delivery starts.
//!   Table sizes sweep 100 → 10k (smallest tier by default; set
//!   `BENCH_SUBSCRIPTION_DELIVERY_SCALING=1` for the full ladder) to
//!   characterise the scan cost.
//!
//! * **Reactive batch** (`DeliverMode::Batch(SubBatchOp)`) — on every
//!   change the server clones `bind`, injects `$event.*`, then executes
//!   the sub-batch and ships its msgpack-encoded `BatchResponse` as the
//!   Event payload. Integrated cost of `inject_event_bindings` +
//!   `execute_reactive_batch` + the inner Find + payload assembly +
//!   push, per change. Single 1-insert variant (the harness owns
//!   repetition count).
//!
//! `DeliverMode::Call` is intentionally NOT benched here: it requires a
//! registered stored function and there's no in-memory funclib fixture
//! the bench can reach from this crate without dragging WASM setup into
//! the bench harness. Reactive::Batch already exercises the
//! `execute_reactive_*` / `$event.*` injection path that Call shares.
//!
//! Everything is built via the typed `shamir-query-builder` API; the
//! frame-capture / mpsc / settle-delay scaffolding mirrors
//! `subscription_throughput.rs` line-for-line so numbers are comparable.
//!
//! Both groups need per-iteration fresh state (a fresh subscription so
//! the registry doesn't accumulate across samples for snapshot; a fresh
//! DB + registry so subscription ids/changefeed watermarks don't
//! accumulate for reactive) — both use `bench_batched_async`.

use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use bench_scale_tool::Harness;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::time::timeout;

use shamir_collections::new_map;
use shamir_connect::common::push_envelope::{PushEnvelope, PushKind};
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::{ConnectionServices, PushRejected, PushSink};
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};

use shamir_db::access::{principal_id, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;

use shamir_query_builder::batch::subscribe::Subscribe;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_query_builder::Query;

use shamir_query_types::batch::{BatchRequest, SubBatchOp};
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
// Actor::User(principal_id("alice")) and needs ownership to pass the gate —
// both setup helpers below stamp that same actor via the `_as` variants.

async fn make_db_one_repo(db: &str, repo: &str, table: &str) -> Arc<ShamirDb> {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let bench_user = Actor::User(principal_id("alice"));
    shamir.create_db_as(db, bench_user.clone()).await;
    let cfg = RepoConfig::new(repo, BoxRepoFactory::in_memory()).add_table(TableConfig::new(table));
    shamir.add_repo_as(db, cfg, bench_user).await.unwrap();
    Arc::new(shamir)
}

async fn make_db_two_tables(db: &str, repo: &str, table_a: &str, table_b: &str) -> Arc<ShamirDb> {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let bench_user = Actor::User(principal_id("alice"));
    shamir.create_db_as(db, bench_user.clone()).await;
    let cfg = RepoConfig::new(repo, BoxRepoFactory::in_memory())
        .add_table(TableConfig::new(table_a))
        .add_table(TableConfig::new(table_b));
    shamir.add_repo_as(db, cfg, bench_user).await.unwrap();
    Arc::new(shamir)
}

/// Submit a subscribe Batch and return the assigned subscription id.
async fn subscribe_and_get_id(
    handler: &ShamirDbHandler,
    session: &Session,
    conn: &ConnectionServices,
    db: &str,
    sub_op: shamir_query_types::subscribe::SubscribeOp,
) -> u64 {
    let mut sb = Batch::new();
    sb.id(1);
    sb.subscribe("m", sub_op);
    let resp = decode(
        &handler
            .handle(session, &encode(&execute_built(db, sb.build())), conn)
            .await
            .unwrap(),
    );
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
        other => panic!("expected Batch response, got {:?}", other),
    }
}

/// Drain push frames from `rx` until `target` Event frames for `sub_id`
/// arrive. Returns the number of events drained (and drops other kinds).
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

/// Drain push frames until `target` Events AND a terminal Ready for
/// `sub_id` arrive (snapshot delivery pattern: N Events → Ready).
/// Returns the number of Event frames seen before Ready.
async fn drain_snapshot(
    rx: &mut UnboundedReceiver<Vec<u8>>,
    sub_id: u64,
    target: usize,
    dur: Duration,
) -> (usize, bool) {
    let deadline = tokio::time::Instant::now() + dur;
    let mut got = 0usize;
    let mut ready = false;
    while !ready {
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
        if env.sub != sub_id {
            continue;
        }
        match env.push {
            PushKind::Event => got += 1,
            PushKind::Ready => ready = true,
            _ => {}
        }
    }
    let _ = target; // target only documents expected Event count
    (got, ready)
}

/// Seed `table` with `n` deterministic rows. Uses the typed insert
/// builder — one row per Batch (matches the throughput bench's pattern
/// and keeps each insert independent so the engine doesn't batch-amortize).
async fn seed_table(
    handler: &ShamirDbHandler,
    session: &Session,
    conn: &ConnectionServices,
    db: &str,
    table: &str,
    n: usize,
) {
    for i in 0..n {
        let mut wb = Batch::new();
        wb.id((i as u64) + 1);
        wb.insert(
            "ins",
            insert(table).row(
                doc! { "_id" => format!("k{i}") }
                    .set("thread_id", (i as i64) % 16)
                    .set("body", format!("seed-{i}")),
            ),
        );
        let _ = handler
            .handle(session, &encode(&execute_built(db, wb.build())), conn)
            .await
            .unwrap();
    }
}

// =============================================================================
// Group 1 — Snapshot delivery (Д4)
// =============================================================================

/// Handler + seeded table shared across every iteration of one `n` variant
/// (seeding happens once, outside the timed loop).
struct SnapshotShared {
    handler: Arc<ShamirDbHandler>,
    n: usize,
}

struct SnapshotIterState {
    handler: Arc<ShamirDbHandler>,
    session: Session,
    conn: ConnectionServices,
    registry: Arc<SubscriptionRegistry>,
    rx: UnboundedReceiver<Vec<u8>>,
    sub_id: u64,
    n: usize,
}

async fn snapshot_setup(shared: Arc<SnapshotShared>) -> SnapshotIterState {
    let s = fixture_session();
    // Fresh push channel + registry per iter so the measurement isolates
    // one snapshot delivery and memory doesn't pile up across samples.
    let (push, rx) = capture();
    let registry = Arc::new(SubscriptionRegistry::new());
    let conn = conn_with(push, registry.clone());

    let sub_op = Subscribe::table(TableRef::with_repo("main", "messages"))
        .with_initial()
        .build();
    let sub_id = subscribe_and_get_id(&shared.handler, &s, &conn, "app", sub_op).await;

    SnapshotIterState {
        handler: shared.handler.clone(),
        session: s,
        conn,
        registry,
        rx,
        sub_id,
        n: shared.n,
    }
}

async fn snapshot_routine(mut state: SnapshotIterState) {
    let (got, ready) = drain_snapshot(
        &mut state.rx,
        state.sub_id,
        state.n,
        Duration::from_secs(30),
    )
    .await;
    assert!(ready, "snapshot did not terminate with Ready frame");
    assert_eq!(
        got, state.n,
        "snapshot Event count mismatch: got {got}/{}",
        state.n
    );
    black_box((state.sub_id, got));

    // Tear the subscription down so the registry stays empty.
    let mut ub = Batch::new();
    ub.id(99);
    ub.unsubscribe("u", state.sub_id);
    let _ = state
        .handler
        .handle(
            &state.session,
            &encode(&execute_built("app", ub.build())),
            &state.conn,
        )
        .await
        .unwrap();
    debug_assert_eq!(state.registry.count(), 0);
}

// =============================================================================
// Group 2 — Reactive delivery (Д5: DeliverMode::Batch)
// =============================================================================

/// Inserts per measured call in the reactive-delivery routine.
///
/// Kept at 1: the harness owns repetition count, so an inner loop of N
/// sequential single-row inserts just inflates per-call cost. Each call
/// fires ONE insert → triggers ONE reactive sub-batch execution → drains
/// ONE event. That's the cheap unit of reactive delivery cost.
const REACTIVE_N: usize = 1;

struct ReactiveIterState {
    handler: ShamirDbHandler,
    session: Session,
    conn: ConnectionServices,
    rx: UnboundedReceiver<Vec<u8>>,
    sub_id: u64,
}

async fn reactive_setup() -> ReactiveIterState {
    // Fresh DB + registry per iter — keeps subscription ids and
    // changefeed watermarks from accumulating.
    let shamir = make_db_two_tables("app", "main", "messages", "threads").await;
    let handler = ShamirDbHandler::new(shamir);
    let s = fixture_session();

    // Seed a few `threads` rows so the inner Find has work to do (mirrors
    // the typical reactive pattern: every new message → fetch its thread).
    let (push_seed, _rx_seed) = capture();
    let registry_seed = Arc::new(SubscriptionRegistry::new());
    let conn_seed = conn_with(push_seed, registry_seed);
    for i in 0..8 {
        let mut wb = Batch::new();
        wb.id((i as u64) + 1);
        wb.insert(
            "ins",
            insert("threads")
                .row(doc! { "_id" => format!("t{i}") }.set("title", format!("thread-{i}"))),
        );
        let _ = handler
            .handle(&s, &encode(&execute_built("app", wb.build())), &conn_seed)
            .await
            .unwrap();
    }

    let (push, rx) = capture();
    let registry = Arc::new(SubscriptionRegistry::new());
    let conn = conn_with(push, registry.clone());

    // Build the reactive sub-batch via typed builders: Find all threads
    // on every event.
    let mut inner = Batch::new();
    inner.id(0);
    inner.query("threads", Query::with_repo("main", "threads"));
    inner.return_all();
    let sub_batch = SubBatchOp {
        batch: inner.build(),
        bind: new_map(),
    };

    let sub_op = Subscribe::table(TableRef::with_repo("main", "messages"))
        .deliver_batch(sub_batch)
        .build();
    let sub_id = subscribe_and_get_id(&handler, &s, &conn, "app", sub_op).await;

    // Let the bridge attach.
    tokio::time::sleep(Duration::from_millis(80)).await;

    ReactiveIterState {
        handler,
        session: s,
        conn,
        rx,
        sub_id,
    }
}

async fn reactive_routine(mut state: ReactiveIterState) {
    for i in 0..REACTIVE_N {
        let mut wb = Batch::new();
        wb.id((i as u64) + 1000);
        wb.insert(
            "ins",
            insert("messages").row(
                doc! { "_id" => format!("m{i}") }
                    .set("thread_id", (i as i64) % 8)
                    .set("body", format!("reactive-{i}")),
            ),
        );
        let _ = state
            .handler
            .handle(
                &state.session,
                &encode(&execute_built("app", wb.build())),
                &state.conn,
            )
            .await
            .unwrap();
    }
    let got = drain_events_for(
        &mut state.rx,
        state.sub_id,
        REACTIVE_N,
        Duration::from_secs(30),
    )
    .await;
    assert_eq!(
        got, REACTIVE_N,
        "reactive delivery lost events: got {got}/{REACTIVE_N}"
    );
    black_box(state.sub_id);
}

fn setup_block_on<T>(fut: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(fut)
}

fn main() {
    let mut h = Harness::new("subscription_delivery", env!("CARGO_MANIFEST_DIR"));

    // -- Group 1: snapshot, table-size sweep.
    //
    // Table size is a genuine structural axis: snapshot delivery scans the
    // full table and pushes one Event per row, so cost grows linearly with N.
    // Default = smallest tier only (cheap per call); set
    // BENCH_SUBSCRIPTION_DELIVERY_SCALING=1 to run the full ladder.
    let wide = std::env::var("BENCH_SUBSCRIPTION_DELIVERY_SCALING")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);
    let snapshot_ns: &[usize] = if wide {
        &[100usize, 1_000, 10_000]
    } else {
        &[100usize]
    };
    for &n in snapshot_ns {
        // One-shot seed shared across all iterations of this variant.
        let shamir = setup_block_on(make_db_one_repo("app", "main", "messages"));
        let handler = Arc::new(ShamirDbHandler::new(shamir));
        let seed_sess = fixture_session();
        let (push_seed, _rx_seed) = capture();
        let registry_seed = Arc::new(SubscriptionRegistry::new());
        let conn_seed = conn_with(push_seed, registry_seed);
        setup_block_on(seed_table(
            &handler, &seed_sess, &conn_seed, "app", "messages", n,
        ));

        let shared = Arc::new(SnapshotShared {
            handler: handler.clone(),
            n,
        });

        let id = format!("subscription_snapshot/n_{n}");
        h.bench_batched_async(
            &id,
            move || snapshot_setup(shared.clone()),
            snapshot_routine,
        );
    }

    // -- Group 2: reactive batch inserts, N = 1.
    h.bench_batched_async(
        "subscription_reactive/reactive_batch_insert_1",
        reactive_setup,
        reactive_routine,
    );

    h.run();
}
