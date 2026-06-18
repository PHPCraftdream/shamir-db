//! Live Subscriptions v1.1 — wire-level end-to-end tests.
//!
//! Drives `ShamirDbHandler::handle()` with msgpack-encoded `DbRequest`
//! batches built exclusively via the typed `shamir-query-builder` API,
//! captures server-initiated push frames through a `PushSink` connected
//! to a `tokio::sync::mpsc` channel, and decodes them as `PushEnvelope`.
//!
//! Tests cover: basic subscribe → insert → event delivery; server-side
//! filter eval suppressing non-matching events; the `subscribe!` macro;
//! multiple subscriptions in one batch; unsubscribe stops further pushes;
//! and the multi-repo guard rejects multi-source subscriptions across
//! different repos with code `multi_repo_subscriptions_not_supported`.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::time::timeout;

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
use shamir_query_builder::write::{delete, insert};
use shamir_query_types::batch::BatchRequest;
use shamir_query_types::filter::{Filter, FilterValue};
use shamir_query_types::subscribe::EventMask;
use shamir_query_types::TableRef;

use shamir_server::db_handler::{DbRequest, DbResponse, ShamirDbHandler};
use shamir_server::subscriptions::SubscriptionRegistry;

// ---------- capture-push: drains frames into an mpsc channel ----------

struct CapturePush {
    tx: UnboundedSender<Vec<u8>>,
}

impl PushSink for CapturePush {
    fn try_push(&self, frame: Vec<u8>) -> Result<(), PushRejected> {
        // If the receiver is dropped the test is over — ignore.
        let _ = self.tx.send(frame);
        Ok(())
    }
}

fn capture() -> (Arc<dyn PushSink>, UnboundedReceiver<Vec<u8>>) {
    let (tx, rx) = unbounded_channel();
    (Arc::new(CapturePush { tx }), rx)
}

// ---------- session / db fixtures ----------

fn user_session() -> Session {
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

async fn make_db_one_repo(db: &str, repo: &str, table: &str) -> Arc<ShamirDb> {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    shamir.create_db(db).await;
    let cfg = RepoConfig::new(repo, BoxRepoFactory::in_memory()).add_table(TableConfig::new(table));
    shamir.add_repo(db, cfg).await.expect("add repo");
    Arc::new(shamir)
}

async fn make_db_two_repos(
    db: &str,
    repo_a: &str,
    table_a: &str,
    repo_b: &str,
    table_b: &str,
) -> Arc<ShamirDb> {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    shamir.create_db(db).await;
    let cfg_a =
        RepoConfig::new(repo_a, BoxRepoFactory::in_memory()).add_table(TableConfig::new(table_a));
    shamir.add_repo(db, cfg_a).await.expect("add repo a");
    let cfg_b =
        RepoConfig::new(repo_b, BoxRepoFactory::in_memory()).add_table(TableConfig::new(table_b));
    shamir.add_repo(db, cfg_b).await.expect("add repo b");
    Arc::new(shamir)
}

// ---------- request encoding helpers ----------

fn encode(req: &DbRequest) -> Vec<u8> {
    rmp_serde::to_vec_named(req).expect("encode req")
}
fn decode(bytes: &[u8]) -> DbResponse {
    rmp_serde::from_slice(bytes).expect("decode response")
}
fn execute_built(db: &str, batch: BatchRequest) -> DbRequest {
    DbRequest::Execute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: db.to_string(),
        batch,
    }
}

fn conn_with(push: Arc<dyn PushSink>, registry: Arc<SubscriptionRegistry>) -> ConnectionServices {
    ConnectionServices {
        conn_id: 1,
        push: Some(push),
        extensions: Some(registry as Arc<dyn std::any::Any + Send + Sync>),
    }
}

/// Decode a `BatchResponse` from a successful `DbResponse::Batch` or panic.
fn unwrap_batch(resp: DbResponse) -> shamir_query_types::batch::BatchResponse {
    match resp {
        DbResponse::Batch { response } => response,
        other => panic!("expected DbResponse::Batch, got {:?}", other),
    }
}

/// Pull the `sub` id the server injected into `results[alias].value.sub`.
fn sub_id_of(resp: &shamir_query_types::batch::BatchResponse, alias: &str) -> u64 {
    use shamir_types::types::value::QueryValue;
    let qr = resp
        .results
        .get(alias)
        .unwrap_or_else(|| panic!("missing result for alias {alias}"));
    let v = qr.value.as_ref().expect("subscribe result has no `value`");
    match v {
        QueryValue::Map(m) => match m.get("sub") {
            Some(QueryValue::Int(id)) => *id as u64,
            other => panic!("subscribe result `sub` is not Int: {:?}", other),
        },
        other => panic!("subscribe result value is not Map: {:?}", other),
    }
}

/// Wait up to `dur` for a single push frame.
async fn next_push(rx: &mut UnboundedReceiver<Vec<u8>>, dur: Duration) -> Option<PushEnvelope> {
    let frame = timeout(dur, rx.recv()).await.ok().flatten()?;
    Some(rmp_serde::from_slice(&frame).expect("decode push envelope"))
}

/// Wait for the first `Event` frame for `sub_id`, draining other kinds
/// (`Ready`, `Gap`, `Closed`, `SlowConsumer`) up to `dur` total.
async fn next_event_for(
    rx: &mut UnboundedReceiver<Vec<u8>>,
    sub_id: u64,
    dur: Duration,
) -> Option<PushEnvelope> {
    let deadline = tokio::time::Instant::now() + dur;
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return None;
        }
        let remaining = deadline - now;
        let env = next_push(rx, remaining).await?;
        if env.sub == sub_id && env.push == PushKind::Event {
            return Some(env);
        }
        // Otherwise keep draining.
    }
}

/// Bridge tasks subscribe to the changefeed asynchronously after the
/// subscribe response is sent. Give them a moment to attach before the
/// next write batch fires events.
async fn settle() {
    tokio::time::sleep(Duration::from_millis(80)).await;
}

// ============================================================================
// 1. basic subscribe → INSERT → Event push
// ============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn basic_subscribe_yields_event_on_insert() {
    let shamir = make_db_one_repo("app", "main", "messages").await;
    let handler = ShamirDbHandler::new(shamir);
    let s = user_session();

    let (push, mut rx) = capture();
    let registry = Arc::new(SubscriptionRegistry::new());
    let conn = conn_with(push, registry.clone());

    // Subscribe via the manual builder.
    let sub_op = Subscribe::table(TableRef::with_repo("main", "messages")).build();
    let mut sb = Batch::new();
    sb.id(1);
    sb.subscribe("m", sub_op);

    let resp = unwrap_batch(decode(
        &handler
            .handle(&s, &encode(&execute_built("app", sb.build())), &conn)
            .await
            .unwrap(),
    ));
    let sub_id = sub_id_of(&resp, "m");
    assert_eq!(registry.count(), 1);

    settle().await;

    // Fire an insert — must emit at least one Event push for this sub.
    let mut wb = Batch::new();
    wb.id(2);
    wb.insert(
        "ins",
        insert("messages").row(doc! { "_id" => "k1", "thread_id" => 1, "body" => "hi" }),
    );
    let _ = handler
        .handle(&s, &encode(&execute_built("app", wb.build())), &conn)
        .await
        .unwrap();

    let env = next_event_for(&mut rx, sub_id, Duration::from_millis(500))
        .await
        .expect("no Event push captured");
    assert_eq!(env.push, PushKind::Event);
    assert_eq!(env.sub, sub_id);

    // Regression guard: the unfiltered Put path must also ship a de-interned
    // `value` object in the Event payload. If `make_event_data` ever drops
    // back to raw interned-key msgpack, the JSON decode below will lose
    // `value` and this assertion will catch it.
    let data_bytes = env.data.as_ref().expect("Event frame missing data");
    let payload: serde_json::Value =
        serde_json::from_slice(data_bytes).expect("Event data is not JSON");
    let value = payload
        .get("value")
        .expect("Event payload missing `value` field (interner-decode bug?)");
    let obj = value
        .as_object()
        .expect("Event `value` is not a string-keyed object");
    assert_eq!(
        obj.get("thread_id").and_then(|n| n.as_i64()),
        Some(1),
        "Event `value` must contain de-interned thread_id=1, got {:?}",
        obj
    );
}

// ============================================================================
// 2. EventMask suppresses non-matching event kinds (server-side gating)
// ============================================================================
//
// The wire-level "filter drops unmatched events" guarantee is exercised here
// via the EventMask discriminator (Put vs Delete). A `Delete`-only
// subscription must NOT receive Event pushes for inserts (Put), and must
// receive an Event push when a row is deleted. This proves the bridge's
// per-event server-side filtering keeps the wire empty for non-matches
// without depending on field-level value introspection of the changefeed
// payload (which the engine ships in interned-key MessagePack form).

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn filter_drops_unmatched_events() {
    let shamir = make_db_one_repo("app", "main", "messages").await;
    let handler = ShamirDbHandler::new(shamir);
    let s = user_session();

    let (push, mut rx) = capture();
    let registry = Arc::new(SubscriptionRegistry::new());
    let conn = conn_with(push, registry.clone());

    // Seed a row BEFORE subscribing (subscription should not get its Put).
    let mut seed = Batch::new();
    seed.id(0);
    seed.insert(
        "seed",
        insert("messages").row(doc! { "_id" => "k1", "thread_id" => 7, "body" => "nope" }),
    );
    let _ = handler
        .handle(&s, &encode(&execute_built("app", seed.build())), &conn)
        .await
        .unwrap();

    // Subscribe with `events: Delete` — only delete changes must be pushed.
    let src = SourceBuilder::table(TableRef::with_repo("main", "messages"))
        .events(EventMask::Delete)
        .build();
    let sub_op = Subscribe::source(src).build();
    let mut sb = Batch::new();
    sb.id(1);
    sb.subscribe("m", sub_op);
    let resp = unwrap_batch(decode(
        &handler
            .handle(&s, &encode(&execute_built("app", sb.build())), &conn)
            .await
            .unwrap(),
    ));
    let sub_id = sub_id_of(&resp, "m");

    settle().await;

    // Insert (Put) — must NOT yield an Event for this Delete-only subscription.
    let mut wb = Batch::new();
    wb.id(2);
    wb.insert(
        "ins",
        insert("messages").row(doc! { "_id" => "k2", "thread_id" => 42, "body" => "yes" }),
    );
    let _ = handler
        .handle(&s, &encode(&execute_built("app", wb.build())), &conn)
        .await
        .unwrap();

    let leak = next_event_for(&mut rx, sub_id, Duration::from_millis(300)).await;
    assert!(
        leak.is_none(),
        "Delete-only subscription must suppress Put events, got {:?}",
        leak
    );

    // Delete the seed row — must yield exactly one Event push.
    let mut db_op = Batch::new();
    db_op.id(3);
    db_op.delete(
        "del",
        delete("messages").where_(Filter::Eq {
            field: vec!["_id".into()],
            value: FilterValue::String("k1".into()),
        }),
    );
    let _ = handler
        .handle(&s, &encode(&execute_built("app", db_op.build())), &conn)
        .await
        .unwrap();

    let env = next_event_for(&mut rx, sub_id, Duration::from_millis(500))
        .await
        .expect("Delete-only subscription missed the delete Event");
    assert_eq!(env.push, PushKind::Event);
    assert_eq!(env.sub, sub_id);
}

// ============================================================================
// 3. subscribe! macro — drives an end-to-end Event delivery
// ============================================================================
//
// The declarative `subscribe!` macro is exercised here against the same
// handler the manual builder uses. The macro's `where:` parameter is
// required by its grammar, so a trivially-satisfied filter is paired with
// `on: delete` so the bridge bypasses field-level filter evaluation
// (filters are only applied to Put changes; for Delete the mask alone
// gates delivery). This proves the macro produces a wire-equivalent
// SubscribeOp that the server accepts and the bridge honours.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subscribe_via_macro() {
    let shamir = make_db_one_repo("app", "main", "messages").await;
    let handler = ShamirDbHandler::new(shamir);
    let s = user_session();

    let (push, mut rx) = capture();
    let registry = Arc::new(SubscriptionRegistry::new());
    let conn = conn_with(push, registry.clone());

    // Seed before subscribing so the row exists at delete-time.
    let mut seed = Batch::new();
    seed.id(0);
    seed.insert(
        "seed",
        insert("messages").row(doc! { "_id" => "macro", "thread_id" => 7 }),
    );
    let _ = handler
        .handle(&s, &encode(&execute_built("app", seed.build())), &conn)
        .await
        .unwrap();

    // Build the subscribe op via the declarative macro.
    let sub_op = shamir_query_builder::subscribe! {
        source: ("main", "messages"),
        where: Filter::Eq {
            field: vec!["thread_id".into()],
            value: FilterValue::Int(7),
        },
        on: delete,
    };

    // Wire-equivalence check: the manually built source produces the same op.
    let manual = Subscribe::source(
        SourceBuilder::table(TableRef::with_repo("main", "messages"))
            .filter(Filter::Eq {
                field: vec!["thread_id".into()],
                value: FilterValue::Int(7),
            })
            .events(EventMask::Delete)
            .build(),
    )
    .build();
    assert_eq!(
        sub_op, manual,
        "subscribe! macro must produce a wire-equivalent SubscribeOp"
    );

    let mut sb = Batch::new();
    sb.id(1);
    sb.subscribe("m", sub_op);
    let resp = unwrap_batch(decode(
        &handler
            .handle(&s, &encode(&execute_built("app", sb.build())), &conn)
            .await
            .unwrap(),
    ));
    let sub_id = sub_id_of(&resp, "m");

    settle().await;

    let mut db_op = Batch::new();
    db_op.id(2);
    db_op.delete(
        "del",
        delete("messages").where_(Filter::Eq {
            field: vec!["_id".into()],
            value: FilterValue::String("macro".into()),
        }),
    );
    let _ = handler
        .handle(&s, &encode(&execute_built("app", db_op.build())), &conn)
        .await
        .unwrap();

    let env = next_event_for(&mut rx, sub_id, Duration::from_millis(500))
        .await
        .expect("macro-built subscription did not deliver Event");
    assert_eq!(env.sub, sub_id);
    assert_eq!(env.push, PushKind::Event);
}

// ============================================================================
// 4. two subscriptions in one batch — distinct sub ids, independent routing
// ============================================================================
//
// Two subs on the same table differ only in their EventMask: one on Put,
// one on Delete. Each insert/delete must reach exactly one sub.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_subscriptions_in_one_batch() {
    let shamir = make_db_one_repo("app", "main", "messages").await;
    let handler = ShamirDbHandler::new(shamir);
    let s = user_session();

    let (push, mut rx) = capture();
    let registry = Arc::new(SubscriptionRegistry::new());
    let conn = conn_with(push, registry.clone());

    let src_put = SourceBuilder::table(TableRef::with_repo("main", "messages"))
        .events(EventMask::Put)
        .build();
    let src_del = SourceBuilder::table(TableRef::with_repo("main", "messages"))
        .events(EventMask::Delete)
        .build();

    let mut sb = Batch::new();
    sb.id(1);
    sb.subscribe("a", Subscribe::source(src_put).build());
    sb.subscribe("b", Subscribe::source(src_del).build());
    let resp = unwrap_batch(decode(
        &handler
            .handle(&s, &encode(&execute_built("app", sb.build())), &conn)
            .await
            .unwrap(),
    ));
    let sub_a = sub_id_of(&resp, "a");
    let sub_b = sub_id_of(&resp, "b");
    assert_ne!(sub_a, sub_b);
    assert_eq!(registry.count(), 2);

    settle().await;

    // INSERT → only the Put-mask sub fires.
    let mut wb = Batch::new();
    wb.id(2);
    wb.insert(
        "ins",
        insert("messages").row(doc! { "_id" => "x1", "thread_id" => 1 }),
    );
    let _ = handler
        .handle(&s, &encode(&execute_built("app", wb.build())), &conn)
        .await
        .unwrap();

    let ev_a = next_event_for(&mut rx, sub_a, Duration::from_millis(500))
        .await
        .expect("Put-mask sub got no Event on insert");
    assert_eq!(ev_a.sub, sub_a);

    // DELETE → only the Delete-mask sub fires.
    let mut db_op = Batch::new();
    db_op.id(3);
    db_op.delete(
        "del",
        delete("messages").where_(Filter::Eq {
            field: vec!["_id".into()],
            value: FilterValue::String("x1".into()),
        }),
    );
    let _ = handler
        .handle(&s, &encode(&execute_built("app", db_op.build())), &conn)
        .await
        .unwrap();

    let ev_b = next_event_for(&mut rx, sub_b, Duration::from_millis(500))
        .await
        .expect("Delete-mask sub got no Event on delete");
    assert_eq!(ev_b.sub, sub_b);
}

// ============================================================================
// 5. unsubscribe stops further pushes
// ============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsubscribe_stops_push() {
    let shamir = make_db_one_repo("app", "main", "messages").await;
    let handler = ShamirDbHandler::new(shamir);
    let s = user_session();

    let (push, mut rx) = capture();
    let registry = Arc::new(SubscriptionRegistry::new());
    let conn = conn_with(push, registry.clone());

    let sub_op = Subscribe::table(TableRef::with_repo("main", "messages")).build();
    let mut sb = Batch::new();
    sb.id(1);
    sb.subscribe("m", sub_op);
    let resp = unwrap_batch(decode(
        &handler
            .handle(&s, &encode(&execute_built("app", sb.build())), &conn)
            .await
            .unwrap(),
    ));
    let sub_id = sub_id_of(&resp, "m");

    settle().await;

    // First insert → one push.
    let mut wb = Batch::new();
    wb.id(2);
    wb.insert(
        "ins",
        insert("messages").row(doc! { "_id" => "first", "thread_id" => 1 }),
    );
    let _ = handler
        .handle(&s, &encode(&execute_built("app", wb.build())), &conn)
        .await
        .unwrap();

    let env = next_event_for(&mut rx, sub_id, Duration::from_millis(500))
        .await
        .expect("first event not delivered");
    assert_eq!(env.sub, sub_id);

    // Issue Unsubscribe through the builder.
    let mut ub = Batch::new();
    ub.id(3);
    ub.unsubscribe("u", sub_id);
    let _ = handler
        .handle(&s, &encode(&execute_built("app", ub.build())), &conn)
        .await
        .unwrap();
    assert_eq!(
        registry.count(),
        0,
        "registry must drop the entry on unsubscribe"
    );

    // Drain any tail frames (e.g. Closed) the cancelled task may emit.
    let drain_until = tokio::time::Instant::now() + Duration::from_millis(150);
    while tokio::time::Instant::now() < drain_until {
        if timeout(Duration::from_millis(20), rx.recv()).await.is_err() {
            break;
        }
    }

    // Second insert — must NOT yield any further Event for this sub.
    let mut wb2 = Batch::new();
    wb2.id(4);
    wb2.insert(
        "ins",
        insert("messages").row(doc! { "_id" => "second", "thread_id" => 1 }),
    );
    let _ = handler
        .handle(&s, &encode(&execute_built("app", wb2.build())), &conn)
        .await
        .unwrap();

    let leaked = next_event_for(&mut rx, sub_id, Duration::from_millis(300)).await;
    assert!(
        leaked.is_none(),
        "unsubscribed bridge must not deliver further events, got {:?}",
        leaked
    );
}

// ============================================================================
// 6. Put + value-filter is honoured (regression: interned-key decode bug)
// ============================================================================
//
// The changefeed ships records as MessagePack with `u64` interned map keys
// (`InnerValue`). A direct `serde_json::Value` decode of those bytes fails
// (JSON requires string keys), so the bridge MUST de-intern through the
// table's interner before filter evaluation and before writing the `value`
// field of the Event payload. This test exercises both ends:
//
//   * an unmatched Put (thread_id=7) must NOT push (filter rejects it),
//   * a matched Put (thread_id=42) MUST push exactly one Event whose
//     `data.value` round-trips back to a string-keyed object containing
//     `thread_id: 42`. (Pre-fix this assertion failed: `value` was absent
//     because the JSON decode of interned-key msgpack returned Err.)

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn value_filter_on_put_is_respected() {
    let shamir = make_db_one_repo("app", "main", "messages").await;
    let handler = ShamirDbHandler::new(shamir);
    let s = user_session();

    let (push, mut rx) = capture();
    let registry = Arc::new(SubscriptionRegistry::new());
    let conn = conn_with(push, registry.clone());

    // Subscribe to Put-only with thread_id == 42.
    let src = SourceBuilder::table(TableRef::with_repo("main", "messages"))
        .filter(Filter::Eq {
            field: vec!["thread_id".into()],
            value: FilterValue::Int(42),
        })
        .events(EventMask::Put)
        .build();
    let sub_op = Subscribe::source(src).build();
    let mut sb = Batch::new();
    sb.id(1);
    sb.subscribe("m", sub_op);
    let resp = unwrap_batch(decode(
        &handler
            .handle(&s, &encode(&execute_built("app", sb.build())), &conn)
            .await
            .unwrap(),
    ));
    let sub_id = sub_id_of(&resp, "m");
    settle().await;

    // INSERT with thread_id=7 — must NOT push (filter rejects it).
    let mut wb = Batch::new();
    wb.id(2);
    wb.insert(
        "ins7",
        insert("messages").row(doc! { "_id" => "k7", "thread_id" => 7, "body" => "no" }),
    );
    let _ = handler
        .handle(&s, &encode(&execute_built("app", wb.build())), &conn)
        .await
        .unwrap();
    let leak = next_event_for(&mut rx, sub_id, Duration::from_millis(300)).await;
    assert!(
        leak.is_none(),
        "Put with thread_id=7 must not pass the thread_id=42 filter, got {:?}",
        leak
    );

    // INSERT with thread_id=42 — MUST push exactly one Event whose
    // `data.value` contains the string-keyed object with thread_id: 42.
    let mut wb = Batch::new();
    wb.id(3);
    wb.insert(
        "ins42",
        insert("messages").row(doc! { "_id" => "k42", "thread_id" => 42, "body" => "yes" }),
    );
    let _ = handler
        .handle(&s, &encode(&execute_built("app", wb.build())), &conn)
        .await
        .unwrap();

    let env = next_event_for(&mut rx, sub_id, Duration::from_millis(500))
        .await
        .expect("matching Put produced no Event push");
    assert_eq!(env.push, PushKind::Event);
    assert_eq!(env.sub, sub_id);

    let data_bytes = env.data.as_ref().expect("Event frame missing data");
    let payload: serde_json::Value =
        serde_json::from_slice(data_bytes).expect("Event data is not JSON");
    let value = payload
        .get("value")
        .expect("Event payload missing `value` field (interner-decode bug?)");
    let obj = value
        .as_object()
        .expect("Event `value` is not a string-keyed object");
    assert_eq!(
        obj.get("thread_id").and_then(|n| n.as_i64()),
        Some(42),
        "Event `value` must contain de-interned thread_id=42, got {:?}",
        obj
    );
}

// ============================================================================
// 7. multi-repo subscribe is rejected by the server with a typed code
// ============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multi_repo_subscribe_rejected() {
    let shamir = make_db_two_repos("app", "main", "messages", "other", "events").await;
    let handler = ShamirDbHandler::new(shamir);
    let s = user_session();

    let (push, _rx) = capture();
    let registry = Arc::new(SubscriptionRegistry::new());
    let conn = conn_with(push, registry.clone());

    let src1 = SourceBuilder::table(TableRef::with_repo("main", "messages")).build();
    let src2 = SourceBuilder::table(TableRef::with_repo("other", "events")).build();
    let op = Subscribe::sources(vec![src1, src2]).build();

    let mut sb = Batch::new();
    sb.id(1);
    sb.subscribe("m", op);

    let resp = decode(
        &handler
            .handle(&s, &encode(&execute_built("app", sb.build())), &conn)
            .await
            .unwrap(),
    );
    match resp {
        DbResponse::Error { code, .. } => {
            assert_eq!(code, "multi_repo_subscriptions_not_supported");
        }
        other => panic!("expected multi-repo Error, got {:?}", other),
    }
    assert_eq!(
        registry.count(),
        0,
        "rejected subscribe must not leak into the registry"
    );
}
