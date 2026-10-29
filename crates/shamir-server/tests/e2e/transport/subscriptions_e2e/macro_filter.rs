//! The `subscribe!` macro and value-filtered Put subscriptions.

use std::sync::Arc;
use std::time::Duration;

use shamir_connect::common::push_envelope::PushKind;
use shamir_connect::server::dispatch::RequestHandler;

use shamir_query_builder::batch::subscribe::{SourceBuilder, Subscribe};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::write::{delete, insert};
use shamir_query_types::filter::{Filter, FilterValue};
use shamir_query_types::subscribe::EventMask;
use shamir_query_types::TableRef;

use shamir_server::db_handler::ShamirDbHandler;
use shamir_server::subscriptions::SubscriptionRegistry;

use super::common::{
    capture, conn_with, decode, encode, execute_built, make_db_one_repo, next_event_for, settle,
    sub_id_of, unwrap_batch, user_session,
};

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
    db_op
        .try_delete(
            "del",
            delete("messages").where_(Filter::Eq {
                field: vec!["_id".into()],
                value: FilterValue::String("macro".into()),
            }),
        )
        .unwrap();
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
// 6. Put + value-filter is honoured (regression: interned-key decode bug)
// ============================================================================
//
// The changefeed ships records as MessagePack with `u64` interned map keys
// (`InnerValue`). A direct string-keyed decode of those bytes fails because
// interned keys are numeric, so the bridge MUST de-intern through the
// table's interner before filter evaluation and before writing the `value`
// field of the Event payload. This test exercises both ends:
//
//   * an unmatched Put (thread_id=7) must NOT push (filter rejects it),
//   * a matched Put (thread_id=42) MUST push exactly one Event whose
//     `data.value` round-trips back to a string-keyed object containing
//     `thread_id: 42`. (Pre-fix this assertion failed: `value` was absent
//     because the msgpack decode of interned-key msgpack returned Err.)

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
    let payload: shamir_types::types::value::QueryValue =
        rmp_serde::from_slice(data_bytes).expect("Event data is not msgpack QueryValue");
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
