use shamir_collections::TMap;
use shamir_query_types::batch::BatchOp;
use shamir_query_types::filter::FilterValue;
use shamir_query_types::subscribe::{DeliverMode, EventMask};
use shamir_query_types::TableRef;

use crate::batch::subscribe::{SourceBuilder, Subscribe};
use crate::batch::Batch;
use crate::bind;
use crate::{filter, val};

#[test]
fn subscribe_table_round_trip() {
    let op = Subscribe::table("messages").build();
    let bytes = rmp_serde::to_vec_named(&op).expect("serialize");
    let decoded: shamir_query_types::subscribe::SubscribeOp =
        rmp_serde::from_slice(&bytes).expect("deserialize");
    assert_eq!(decoded, op);
}

#[test]
fn subscribe_with_filter_via_source_builder() {
    let source = SourceBuilder::table(TableRef::with_repo("main", "messages"))
        .filter(filter::eq("status", val::lit("active")))
        .events(EventMask::Put)
        .build();
    let op = Subscribe::source(source).build();
    assert_eq!(op.subscribe.len(), 1);
    assert!(op.subscribe[0].filter.is_some());
    assert_eq!(op.subscribe[0].events, EventMask::Put);
}

#[test]
fn subscribe_all_fields() {
    let op = Subscribe::table("messages")
        .deliver_keys()
        .with_initial()
        .from_version(100)
        .build();
    assert_eq!(op.deliver, DeliverMode::Keys);
    assert!(op.initial);
    assert_eq!(op.from_version, Some(100));
}

#[test]
fn batch_subscribe_op() {
    let mut batch = Batch::new();
    batch.subscribe("sub", Subscribe::table("messages").build());
    let req = batch.build();
    let entry = req.queries.get("sub").expect("missing sub entry");
    assert!(matches!(&entry.op, BatchOp::Subscribe(_)));
}

#[test]
fn batch_unsubscribe_op() {
    let mut batch = Batch::new();
    batch.unsubscribe("unsub", 42);
    let req = batch.build();
    let entry = req.queries.get("unsub").expect("missing unsub entry");
    match &entry.op {
        BatchOp::Unsubscribe(op) => assert_eq!(op.unsubscribe, 42),
        other => panic!("expected Unsubscribe, got {:?}", other),
    }
}

#[test]
fn bind_macro_produces_tmap() {
    let map: TMap<String, FilterValue> = bind! {
        "x" => val::lit(1),
        "y" => val::lit("hello"),
    };
    assert_eq!(map.len(), 2);
    assert!(map.contains_key("x"));
    assert!(map.contains_key("y"));
}
