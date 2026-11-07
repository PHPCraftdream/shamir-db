//! Basic subscribe → event delivery, EventMask gating, and the multi-repo
//! subscribe rejection.

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

use shamir_server::db_handler::{DbResponse, ShamirDbHandler};
use shamir_server::subscriptions::SubscriptionRegistry;

use super::common::{
    capture, conn_with, decode, encode, execute_built, make_db_one_repo, make_db_two_repos,
    next_event_for, settle, sub_id_of, unwrap_batch, user_session,
};

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
    // back to raw interned-key msgpack, the msgpack decode below will lose
    // `value` and this assertion will catch it.
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
    db_op
        .try_delete(
            "del",
            delete("messages").where_(Filter::Eq {
                field: vec!["_id".into()],
                value: FilterValue::String("k1".into()),
            }),
        )
        .unwrap();
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
