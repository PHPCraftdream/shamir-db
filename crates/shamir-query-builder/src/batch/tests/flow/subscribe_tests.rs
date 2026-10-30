use shamir_collections::TMap;
use shamir_query_types::batch::{BatchOp, BatchPlanner, SubBatchOp};
use shamir_query_types::call::CallOp;
use shamir_query_types::filter::FilterValue;
use shamir_query_types::subscribe::{DeliverMode, EventMask};
use shamir_query_types::TableRef;

use crate::batch::subscribe::{SourceBuilder, Subscribe};
use crate::batch::Batch;
use crate::bind;
use crate::{filter, val};

#[test]
fn subscription_scoped_refs_match_planner() {
    let source = SourceBuilder::table(TableRef::new("messages"))
        .filter(filter::eq("id", val::qref_all("inner")))
        .build();
    let mut inner = Batch::new();
    inner.query(
        "child",
        crate::query::Query::from("messages").where_eq("id", val::qref_all("nested")),
    );
    let mut bind = shamir_collections::new_map();
    bind.insert("arg".into(), val::qref_all("binding"));
    let modes = [
        DeliverMode::Records,
        DeliverMode::Keys,
        DeliverMode::Batch(SubBatchOp {
            batch: inner.build(),
            bind,
        }),
        DeliverMode::Call(CallOp {
            call: "notify".into(),
            repo: "main".into(),
            params: vec![val::qref_all("delivery")],
        }),
    ];
    for mode in modes {
        let mut op = Subscribe::source(source.clone()).build();
        op.deliver = mode;
        let mut batch = Batch::new();
        batch.subscribe("sub", op);
        let request = batch.build();
        // The planner's wildcard arm ignores Subscribe, including its sources
        // and all delivery payloads, when extracting outer dependencies.
        assert!(BatchPlanner::plan(&request.queries, &request.limits).is_ok());
        assert!(batch.try_build().is_ok());
    }
}

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
