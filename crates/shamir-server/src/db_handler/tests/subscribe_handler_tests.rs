use std::sync::Arc;

use shamir_collections::TMap;
use shamir_connect::server::conn_services::{ConnectionServices, PushRejected, PushSink};
use shamir_db::access::Actor;
use shamir_db::types::common::new_map;
use shamir_db::types::value::QueryValue;
use shamir_db::ShamirDb;
use shamir_query_types::batch::{
    BatchLimits, BatchOp, BatchRequest, BatchResponse, QueryEntry, ResultEncoding,
};
use shamir_query_types::read::QueryResult;
use shamir_query_types::subscribe::{DeliverMode, SubscribeOp, SubscriptionSource};
use shamir_query_types::TableRef;

use crate::subscriptions::SubscriptionRegistry;

struct NoopPush;

impl PushSink for NoopPush {
    fn try_push(&self, _frame: Vec<u8>) -> Result<(), PushRejected> {
        Ok(())
    }
}

#[tokio::test]
async fn activate_subscriptions_injects_sub_id_into_response() {
    let db = Arc::new(ShamirDb::init_memory().await.unwrap());

    let registry = Arc::new(SubscriptionRegistry::new());
    let push: Arc<dyn PushSink> = Arc::new(NoopPush);

    let conn = ConnectionServices {
        conn_id: 1,
        push: Some(push),
        extensions: Some(registry.clone() as Arc<dyn std::any::Any + Send + Sync>),
    };

    let source = SubscriptionSource {
        table: TableRef {
            repo: "main".into(),
            table: "users".into(),
        },
        filter: None,
        events: Default::default(),
    };

    let subscribe_op = SubscribeOp {
        subscribe: vec![source],
        deliver: DeliverMode::Records,
        initial: false,
        from_version: None,
    };

    let mut queries: TMap<String, QueryEntry> = Default::default();
    queries.insert(
        "my_sub".to_string(),
        QueryEntry {
            op: BatchOp::Subscribe(subscribe_op),
            return_result: true,
            after: Vec::new(),
        },
    );

    let batch = BatchRequest {
        id: QueryValue::Int(1),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    let mut response = BatchResponse {
        id: QueryValue::Int(1),
        results: Default::default(),
        execution_plan: vec![],
        execution_time_us: 0,
        transaction: None,
        interner_delta: Default::default(),
    };
    let mut initial_value = new_map();
    initial_value.insert("subscription_grant".to_string(), QueryValue::Bool(true));
    initial_value.insert("sources_count".to_string(), QueryValue::Int(1));
    response.results.insert(
        "my_sub".to_string(),
        QueryResult {
            records: vec![],
            stats: None,
            pagination: None,
            value: Some(QueryValue::Map(initial_value)),
            explain: None,
        },
    );

    super::super::subscribe_handler::activate_subscriptions(
        &conn,
        &db,
        "test_db",
        &batch,
        &mut response,
        Actor::System,
    );

    let qr = response
        .results
        .get("my_sub")
        .expect("my_sub result missing");
    let value = qr.value.as_ref().expect("value missing");
    let sub_id = match value {
        QueryValue::Map(m) => match m.get("sub") {
            Some(QueryValue::Int(id)) => *id as u64,
            other => panic!("expected sub field as Int, got {:?}", other),
        },
        other => panic!("expected Map value, got {:?}", other),
    };
    assert_eq!(sub_id, 1, "first subscription should get sub_id = 1");
    assert_eq!(registry.count(), 1);
}

#[tokio::test]
async fn activate_subscriptions_enforces_per_connection_cap() {
    // Finding 2b-i: a connection already at its subscription cap must have
    // further Subscribe ops rejected (error injected into the alias result),
    // not silently accepted as new bridge tasks.
    let db = Arc::new(ShamirDb::init_memory().await.unwrap());

    let registry = Arc::new(SubscriptionRegistry::with_cap(2));
    let push: Arc<dyn PushSink> = Arc::new(NoopPush);

    let conn = ConnectionServices {
        conn_id: 1,
        push: Some(push),
        extensions: Some(registry.clone() as Arc<dyn std::any::Any + Send + Sync>),
    };

    let make_source = || SubscriptionSource {
        table: TableRef {
            repo: "main".into(),
            table: "users".into(),
        },
        filter: None,
        events: Default::default(),
    };
    let make_op = || {
        BatchOp::Subscribe(SubscribeOp {
            subscribe: vec![make_source()],
            deliver: DeliverMode::Records,
            initial: false,
            from_version: None,
        })
    };

    // Three Subscribe ops in one batch; cap is 2 → the third is rejected.
    let mut queries: TMap<String, QueryEntry> = Default::default();
    for alias in ["s1", "s2", "s3"] {
        queries.insert(
            alias.to_string(),
            QueryEntry {
                op: make_op(),
                return_result: true,
                after: Vec::new(),
            },
        );
    }

    let batch = BatchRequest {
        id: QueryValue::Int(1),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    let mut response = BatchResponse {
        id: QueryValue::Int(1),
        results: Default::default(),
        execution_plan: vec![],
        execution_time_us: 0,
        transaction: None,
        interner_delta: Default::default(),
    };
    for alias in ["s1", "s2", "s3"] {
        let mut v = new_map();
        v.insert("subscription_grant".to_string(), QueryValue::Bool(true));
        response.results.insert(
            alias.to_string(),
            QueryResult {
                records: vec![],
                stats: None,
                pagination: None,
                value: Some(QueryValue::Map(v)),
                explain: None,
            },
        );
    }

    super::super::subscribe_handler::activate_subscriptions(
        &conn,
        &db,
        "test_db",
        &batch,
        &mut response,
        Actor::System,
    );

    // Exactly 2 subscriptions active (the cap); the 3rd was rejected.
    assert_eq!(registry.count(), 2, "cap must bound active subscriptions");

    // Count how many aliases carry a `sub` id vs an `error`.
    let mut granted = 0usize;
    let mut rejected = 0usize;
    for alias in ["s1", "s2", "s3"] {
        let qr = response.results.get(alias).unwrap();
        if let Some(QueryValue::Map(m)) = qr.value.as_ref() {
            if m.get("sub").is_some() {
                granted += 1;
            }
            if m.get("error").is_some() {
                rejected += 1;
            }
        }
    }
    assert_eq!(granted, 2, "two subscriptions granted");
    assert_eq!(rejected, 1, "one subscription rejected at the cap");
}
