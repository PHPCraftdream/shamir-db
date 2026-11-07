use futures::StreamExt;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;
use shamir_query_types::batch::{
    BatchLimits, BatchOp, BatchRequest, ForEachOp, QueryEntry, ResultEncoding, SubBatchOp,
};
use shamir_query_types::filter::{Cond, Filter, FilterValue};
use shamir_query_types::read::{ReadQuery, Select, SelectExpr, SelectItem};
use shamir_types::access::Actor;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::{execute_batch_unchecked as execute_batch, TableResolver};
use crate::query::TableRef;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::{RepoConfig, RepoInstance};
use crate::table::{TableConfig, TableManager};
use shamir_storage::error::DbResult;

struct TestResolver {
    db: DbInstance,
}

#[async_trait::async_trait]
impl TableResolver for TestResolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        self.db.get_table("default", &table_ref.table).await
    }

    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<RepoInstance> {
        self.db.get_repo("default").ok_or_else(|| {
            shamir_storage::error::DbError::NotFound("repo 'default' not found".into())
        })
    }
}

async fn setup() -> TestResolver {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("users"), TableConfig::new("orders")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    TestResolver { db }
}

fn nested_cond(depth: usize) -> FilterValue {
    let mut value = FilterValue::Int(1);
    for _ in 0..depth {
        value = FilterValue::Cond {
            cond: Box::new(Cond::new(
                Filter::ValueCompare {
                    left: FilterValue::Int(1),
                    cmp: shamir_query_types::filter::ValueCompareOp::Eq,
                    right: FilterValue::Int(1),
                },
                value,
                FilterValue::Int(0),
            )),
        };
    }
    value
}

fn batch_with(queries: shamir_collections::TMap<String, QueryEntry>) -> BatchRequest {
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

fn entry(op: BatchOp) -> QueryEntry {
    QueryEntry {
        op,
        return_result: true,
        after: Vec::new(),
        when: None,
    }
}

#[tokio::test]
async fn rv_engine_c1a53a7a_rejects_deep_foreach_over_even_with_empty_body() {
    let resolver = setup().await;
    let mut queries = new_map();
    queries.insert(
        "loop".into(),
        entry(BatchOp::ForEach(ForEachOp {
            over: nested_cond(70),
            bind_row: "row".into(),
            batch: batch_with(new_map()),
        })),
    );
    let error = execute_batch(
        &batch_with(queries),
        &resolver,
        None,
        None,
        Actor::System,
        "test",
    )
    .await
    .expect_err("over's nested conditional depth must fail before iteration");
    assert!(
        error.to_string().contains("filter nesting depth exceeds"),
        "{error}"
    );
}

#[tokio::test]
async fn rv_engine_c1a53a7a_rejects_invalid_patterns_in_bind_and_select_args() {
    let resolver = setup().await;
    let invalid_cond = || FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::Not {
                filter: Box::new(Filter::Regex {
                    field: vec!["x".into()],
                    pattern: "(".into(),
                }),
            },
            FilterValue::Int(1),
            FilterValue::Int(0),
        )),
    };

    let mut inner_queries = new_map();
    inner_queries.insert("r".into(), entry(BatchOp::Read(ReadQuery::new("users"))));
    let mut binds = new_map();
    binds.insert("arg".into(), invalid_cond());
    let mut queries = new_map();
    queries.insert(
        "sub".into(),
        entry(BatchOp::Batch(SubBatchOp {
            batch: batch_with(inner_queries),
            bind: binds,
        })),
    );
    let error = execute_batch(
        &batch_with(queries),
        &resolver,
        None,
        None,
        Actor::System,
        "test",
    )
    .await
    .expect_err("invalid pattern nested in bind must be rejected at the outer preflight");
    assert!(
        error.to_string().contains("invalid regex pattern"),
        "{error}"
    );

    let mut queries = new_map();
    let read = ReadQuery::new("users").select(Select {
        items: vec![SelectItem::Function {
            name: "strings/upper".into(),
            args: vec![invalid_cond()],
            alias: Some("result".into()),
        }],
        distinct: false,
    });
    queries.insert("read".into(), entry(BatchOp::Read(read)));
    let error = execute_batch(
        &batch_with(queries),
        &resolver,
        None,
        None,
        Actor::System,
        "test",
    )
    .await
    .expect_err("invalid pattern nested in select function args must be rejected preflight");
    assert!(
        error.to_string().contains("invalid regex pattern"),
        "{error}"
    );
}

#[tokio::test]
async fn rv_engine_c1a53a7a_rejects_deep_where_before_prior_insert() {
    let resolver = setup().await;
    let mut outer = new_map();
    let built_insert = {
        let mut builder = Batch::new();
        builder.insert(
            "before",
            shamir_query_builder::write::insert("users").row(
                shamir_query_builder::write::doc()
                    .set("name", "before")
                    .unwrap(),
            ),
        );
        builder.build()
    };
    let insert_entry = built_insert
        .queries
        .into_iter()
        .next()
        .expect("built insert entry")
        .1;
    outer.insert("before".into(), insert_entry);
    outer.insert(
        "nested".into(),
        QueryEntry {
            op: BatchOp::Batch(SubBatchOp {
                batch: batch_with({
                    let mut body = new_map();
                    body.insert(
                        "deep".into(),
                        entry(BatchOp::Read(ReadQuery::from(Query::from("users").where_(
                            Filter::Eq {
                                field: vec!["x".into()],
                                value: nested_cond(70),
                            },
                        )))),
                    );
                    body
                }),
                bind: new_map(),
            }),
            return_result: true,
            after: vec!["before".into()],
            when: None,
        },
    );
    let error = execute_batch(
        &batch_with(outer),
        &resolver,
        None,
        None,
        Actor::System,
        "test",
    )
    .await
    .expect_err("nested deep body WHERE must fail before preceding insert");
    assert!(
        error.to_string().contains("filter nesting depth exceeds"),
        "{error}"
    );
    let table = resolver.db.get_table("default", "users").await.unwrap();
    let mut stream = table.list_stream(64);
    let mut row_count = 0usize;
    while let Some(records) = stream.next().await {
        row_count += records.unwrap().len();
    }
    assert_eq!(
        row_count, 0,
        "the earlier non-empty insert must not execute if validation is preflight",
    );
}

#[test]
fn rv_engine_c1a53a7a_select_expr_depth_guard_is_iterative_surface() {
    let mut expr = SelectExpr::Literal {
        value: shamir_query_types::read::SelectExprValue::Int(1),
    };
    for _ in 0..70 {
        expr = SelectExpr::Add {
            left: Box::new(expr),
            right: Box::new(SelectExpr::Literal {
                value: shamir_query_types::read::SelectExprValue::Int(1),
            }),
        };
    }
    assert!(shamir_query_types::filter::check_select_expr_depth(&expr).is_err());
}
