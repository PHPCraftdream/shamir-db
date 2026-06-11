use std::sync::Arc;

use serde_json::json;
use shamir_collections::TMap;
use shamir_connect::server::conn_services::{ConnectionServices, PushRejected, PushSink};
use shamir_db::access::Actor;
use shamir_db::ShamirDb;
use shamir_query_types::batch::{BatchLimits, BatchOp, BatchRequest, BatchResponse, QueryEntry};
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
        id: json!(1),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
    };

    let mut response = BatchResponse {
        id: json!(1),
        results: Default::default(),
        execution_plan: vec![],
        execution_time_us: 0,
        transaction: None,
    };
    response.results.insert(
        "my_sub".to_string(),
        QueryResult {
            records: vec![],
            stats: None,
            pagination: None,
            value: Some(json!({
                "subscription_grant": true,
                "sources_count": 1
            })),
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
    let sub_id = value.get("sub").expect("sub field missing");
    assert_eq!(sub_id, 1, "first subscription should get sub_id = 1");
    assert_eq!(registry.count(), 1);
}
