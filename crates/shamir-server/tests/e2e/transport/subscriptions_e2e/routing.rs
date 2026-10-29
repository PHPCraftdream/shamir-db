//! Multi-subscription routing and unsubscribe semantics.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::timeout;

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
    db_op
        .try_delete(
            "del",
            delete("messages").where_(Filter::Eq {
                field: vec!["_id".into()],
                value: FilterValue::String("x1".into()),
            }),
        )
        .unwrap();
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
