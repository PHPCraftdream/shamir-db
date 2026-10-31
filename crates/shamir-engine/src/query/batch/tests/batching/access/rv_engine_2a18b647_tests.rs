use shamir_query_types::admin::types::schema_ops::{
    AddSchemaRuleOp, ConstraintsDto, FieldRuleDto, SetTableSchemaOp,
};
use shamir_query_types::batch::{BatchLimits, BatchOp, BatchRequest, QueryEntry, ResultEncoding};
use shamir_query_types::call::CallOp;
use shamir_query_types::filter::{Cond, Filter, FilterValue};
use shamir_query_types::read::QueryResult;
use shamir_types::access::Actor;
use shamir_types::types::common::{new_map, TMap};
use shamir_types::types::value::QueryValue;

use super::filter_guard_tests::{setup, TestResolver};
use crate::query::batch::{
    execute_batch_unchecked as execute_batch, AdminExecutor, BatchError, FunctionInvoker,
};
use shamir_query_types::subscribe::{SubscribeOp, SubscriptionSource};
use shamir_query_types::TableRef;

/// No-op admin executor: schema ops must fail at VALIDATION time (pattern
/// guard), so a dummy that would accept anything rules out the unrelated
/// "Admin operations not supported in this context" dispatch error and
/// makes the negative tests red on pre-fix code.
struct NullAdmin;

#[async_trait::async_trait]
impl AdminExecutor for NullAdmin {
    async fn execute_admin(&self, _op: &BatchOp) -> Result<QueryResult, BatchError> {
        Ok(QueryResult::default())
    }
}

/// No-op invoker: without it `BatchOp::Call` fails with "Function calls not
/// supported" for an unrelated reason and the pattern-guard tests stay green
/// on pre-fix code (the guard is never reached).
struct NullInvoker;

#[async_trait::async_trait]
impl FunctionInvoker for NullInvoker {
    async fn invoke_call(
        &self,
        _op: &CallOp,
        _actor: &Actor,
        _resolved_refs: &TMap<String, QueryResult>,
    ) -> Result<QueryResult, BatchError> {
        Ok(QueryResult::default())
    }
}

fn request(op: BatchOp) -> BatchRequest {
    let mut queries = new_map();
    queries.insert(
        "surface".into(),
        QueryEntry {
            op,
            return_result: true,
            after: vec![],
            when: None,
        },
    );
    BatchRequest {
        id: QueryValue::Int(9),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    }
}

fn invalid_cond() -> FilterValue {
    FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::Regex {
                field: vec!["x".into()],
                pattern: "(bad".into(),
            },
            FilterValue::Bool(true),
            FilterValue::Bool(false),
        )),
    }
}

async fn rejected(resolver: &TestResolver, op: BatchOp) {
    let admin = NullAdmin;
    let invoker = NullInvoker;
    let err = execute_batch(
        &request(op),
        resolver,
        Some(&admin),
        Some(&invoker),
        Actor::System,
        "test",
    )
    .await
    .unwrap_err();
    match &err {
        BatchError::QueryError { alias, message, .. } => {
            assert_eq!(alias, "surface");
            // Distinguish the pattern-guard failure from the old dispatch-time
            // errors ("Function calls not supported" / "Admin operations not
            // supported in this context") — this is what makes the test red
            // on pre-fix code.
            assert!(
                message.contains("regex pattern") || message.contains("exceeds"),
                "expected a pattern-guard message, got {message:?}"
            );
        }
        other => panic!("expected QueryError, got {other:?}"),
    }
}

#[tokio::test]
async fn rv_engine_2a18b647_invalid_pattern_in_call_params_rejected() {
    let resolver = setup().await;
    rejected(
        &resolver,
        BatchOp::Call(shamir_query_types::call::CallOp {
            call: "f".into(),
            params: vec![invalid_cond()],
            repo: "main".into(),
        }),
    )
    .await;
}

#[tokio::test]
async fn rv_engine_2a18b647_over_cap_pattern_in_call_params_rejected() {
    let resolver = setup().await;
    let value = FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::Regex {
                field: vec!["x".into()],
                pattern: "x".repeat(crate::query::filter::MAX_FILTER_PATTERN_LENGTH + 1),
            },
            FilterValue::Bool(true),
            FilterValue::Bool(false),
        )),
    };
    rejected(
        &resolver,
        BatchOp::Call(shamir_query_types::call::CallOp {
            call: "f".into(),
            params: vec![value],
            repo: "main".into(),
        }),
    )
    .await;
}

#[tokio::test]
async fn rv_engine_2a18b647_invalid_pattern_in_subscribe_filter_rejected() {
    let resolver = setup().await;
    let op = BatchOp::Subscribe(SubscribeOp {
        subscribe: vec![SubscriptionSource {
            table: TableRef::new("users"),
            filter: Some(Filter::Regex {
                field: vec!["x".into()],
                pattern: "(bad".into(),
            }),
            events: Default::default(),
        }],
        deliver: Default::default(),
        initial: false,
        from_version: None,
    });
    rejected(&resolver, op).await;
}

#[tokio::test]
async fn rv_engine_2a18b647_over_cap_pattern_in_subscribe_filter_rejected() {
    let resolver = setup().await;
    let op = BatchOp::Subscribe(SubscribeOp {
        subscribe: vec![SubscriptionSource {
            table: TableRef::new("users"),
            filter: Some(Filter::Regex {
                field: vec!["x".into()],
                pattern: "x".repeat(crate::query::filter::MAX_FILTER_PATTERN_LENGTH + 1),
            }),
            events: Default::default(),
        }],
        deliver: Default::default(),
        initial: false,
        from_version: None,
    });
    rejected(&resolver, op).await;
}

#[tokio::test]
async fn rv_engine_2a18b647_valid_pattern_in_subscribe_filter_passes() {
    let resolver = setup().await;
    let op = BatchOp::Subscribe(SubscribeOp {
        subscribe: vec![SubscriptionSource {
            table: TableRef::new("users"),
            filter: Some(Filter::Regex {
                field: vec!["x".into()],
                pattern: "^ok".into(),
            }),
            events: Default::default(),
        }],
        deliver: Default::default(),
        initial: false,
        from_version: None,
    });
    execute_batch(&request(op), &resolver, None, None, Actor::System, "test")
        .await
        .expect("a valid pattern in a Subscribe filter must pass validation");
}

fn cond_default(pattern: &str) -> ConstraintsDto {
    ConstraintsDto {
        default: Some(FilterValue::Cond {
            cond: Box::new(Cond::new(
                Filter::Regex {
                    field: vec!["x".into()],
                    pattern: pattern.into(),
                },
                FilterValue::Bool(true),
                FilterValue::Bool(false),
            )),
        }),
        ..ConstraintsDto::default()
    }
}

#[tokio::test]
async fn rv_engine_2a18b647_invalid_pattern_in_set_table_schema_default_rejected() {
    let resolver = setup().await;
    let op = BatchOp::SetTableSchema(SetTableSchemaOp {
        set_table_schema: "users".into(),
        repo: "main".into(),
        schema: vec![FieldRuleDto {
            path: vec!["x".into()],
            r#type: "bool".into(),
            constraints: cond_default("(bad"),
            keyset_safe: false,
        }],
        expected_version: None,
    });
    rejected(&resolver, op).await;
}

#[tokio::test]
async fn rv_engine_2a18b647_over_cap_pattern_in_add_schema_rule_default_rejected() {
    let resolver = setup().await;
    let op = BatchOp::AddSchemaRule(AddSchemaRuleOp {
        add_schema_rule: "users".into(),
        repo: "main".into(),
        rule: FieldRuleDto {
            path: vec!["x".into()],
            r#type: "bool".into(),
            constraints: cond_default(
                &"x".repeat(crate::query::filter::MAX_FILTER_PATTERN_LENGTH + 1),
            ),
            keyset_safe: false,
        },
    });
    rejected(&resolver, op).await;
}

#[tokio::test]
async fn rv_engine_2a18b647_valid_pattern_in_schema_defaults_passes() {
    let resolver = setup().await;
    let set = BatchOp::SetTableSchema(SetTableSchemaOp {
        set_table_schema: "users".into(),
        repo: "main".into(),
        schema: vec![FieldRuleDto {
            path: vec!["x".into()],
            r#type: "bool".into(),
            constraints: cond_default("^ok"),
            keyset_safe: false,
        }],
        expected_version: None,
    });
    let admin = NullAdmin;
    execute_batch(
        &request(set),
        &resolver,
        Some(&admin),
        None,
        Actor::System,
        "test",
    )
    .await
    .expect("a valid pattern in a SetTableSchema default must pass validation");

    let add = BatchOp::AddSchemaRule(AddSchemaRuleOp {
        add_schema_rule: "users".into(),
        repo: "main".into(),
        rule: FieldRuleDto {
            path: vec!["y".into()],
            r#type: "bool".into(),
            constraints: cond_default("^ok"),
            keyset_safe: false,
        },
    });
    let admin = NullAdmin;
    execute_batch(
        &request(add),
        &resolver,
        Some(&admin),
        None,
        Actor::System,
        "test",
    )
    .await
    .expect("a valid pattern in an AddSchemaRule default must pass validation");
}
