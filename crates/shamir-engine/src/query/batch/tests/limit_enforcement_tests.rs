//! W3 regressions: request/body limits may lower, but never raise root caps.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::write::{doc, insert};
use shamir_query_builder::Query;
use shamir_query_types::batch::{BatchLimits, BatchRequest, ExecPolicy, LimitKind};
use shamir_query_types::filter::FilterValue;
use shamir_types::access::Actor;

use crate::query::batch::{
    execute_batch, execute_in_open_tx, open_interactive_tx, Authorized, BatchError, BatchResponse,
    TableResolver,
};
use crate::query::TableRef;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoInstance;
use crate::table::{TableConfig, TableManager};

struct Resolver {
    repo: RepoInstance,
}

#[async_trait::async_trait]
impl TableResolver for Resolver {
    async fn resolve(&self, table: &TableRef) -> shamir_storage::error::DbResult<TableManager> {
        self.repo.get_table(&table.table).await
    }

    async fn resolve_repo(&self, _: &str) -> shamir_storage::error::DbResult<RepoInstance> {
        Ok(self.repo.clone())
    }
}

async fn setup() -> Resolver {
    Resolver {
        repo: RepoInstance::from_factory(
            "default".into(),
            BoxRepoFactory::in_memory(),
            vec![TableConfig::new("users")],
        )
        .await
        .unwrap(),
    }
}

// 0 = ordinary execution, 1 = closed transaction, 2 = interactive transaction.
async fn run(
    request: &BatchRequest,
    resolver: &Resolver,
    policy: ExecPolicy,
    mode: usize,
) -> Result<BatchResponse, BatchError> {
    let mut request = request.clone();
    request.transactional = mode == 1;
    let auth = Authorized::unchecked(&request, Actor::System, "test").with_policy(policy);
    if mode == 2 {
        let (mut tx, _guard) =
            open_interactive_tx(&resolver.repo, shamir_tx::IsolationLevel::Snapshot)
                .await
                .unwrap();
        execute_in_open_tx(auth, resolver, None, None, &mut tx).await
    } else {
        execute_batch(auth, resolver, None, None).await
    }
}

fn reads(count: usize, limits: BatchLimits) -> BatchRequest {
    let mut batch = Batch::new();
    batch.limits(limits);
    for i in 0..count {
        batch.query(format!("r{i}"), Query::from("users"));
    }
    batch.build()
}

// Red before W3: the nested planner sees usize::MAX instead of root 50.
#[tokio::test]
async fn limit_enforcement_nested_query_count_cannot_raise_root() {
    let resolver = setup().await;
    let body = reads(
        60,
        BatchLimits {
            max_queries: usize::MAX,
            ..BatchLimits::default()
        },
    );
    let mut batch = Batch::new();
    batch.sub_batch_no_bind("nested", body);
    for mode in 0..3 {
        assert_eq!(
            run(&batch.build(), &resolver, ExecPolicy::EMBEDDED, mode)
                .await
                .unwrap_err(),
            BatchError::TooManyQueries { count: 60, max: 50 },
        );
    }
}

// Red before W3: three iterations run and commit writes on the ordinary path.
#[tokio::test]
async fn limit_enforcement_loop_rejects_before_first_iteration() {
    let resolver = setup().await;
    let mut body = Batch::new();
    body.limits(BatchLimits {
        max_iterations: usize::MAX,
        ..BatchLimits::default()
    });
    body.insert("ins", insert("users").row(doc().set("name", "unexpected")));
    let mut batch = Batch::new();
    batch.limits(BatchLimits {
        max_iterations: 2,
        ..BatchLimits::default()
    });
    batch.for_each(
        "loop",
        FilterValue::Array(vec![1.into(), 2.into(), 3.into()]),
        "row",
        body.build(),
    );
    for mode in 0..3 {
        assert_eq!(
            run(&batch.build(), &resolver, ExecPolicy::EMBEDDED, mode)
                .await
                .unwrap_err(),
            BatchError::TooManyIterations {
                alias: "loop".into(),
                actual: 3,
                max: 2,
            },
        );
    }
    let response = run(
        &reads(1, BatchLimits::default()),
        &resolver,
        ExecPolicy::EMBEDDED,
        0,
    )
    .await
    .unwrap();
    assert!(
        response.results["r0"].records.is_empty(),
        "no iteration may write",
    );
}

// Red before W3: policy page-size caps are not used by pagination validation.
#[tokio::test]
async fn limit_enforcement_policy_page_size_boundary() {
    let resolver = setup().await;
    let policy = ExecPolicy {
        caps: BatchLimits {
            max_page_size: 10,
            ..BatchLimits::CEILING
        },
        enforce_result_size: false,
    };
    for mode in 0..3 {
        for limit in [11, 10] {
            let mut batch = Batch::new();
            batch.query("r", Query::from("users").limit(limit));
            let result = run(&batch.build(), &resolver, policy.clone(), mode).await;
            if limit == 11 {
                assert_eq!(
                    result.unwrap_err(),
                    BatchError::LimitExceeded {
                        kind: LimitKind::PageSize,
                        actual: 11,
                        max: 10,
                    },
                );
            } else {
                let response = result.unwrap();
                assert_eq!(response.results.len(), 1);
                assert!(response.results["r"].records.is_empty());
            }
        }
    }
}

#[tokio::test]
async fn limit_enforcement_offset_boundary() {
    let resolver = setup().await;
    for mode in 0..3 {
        let mut batch = Batch::new();
        batch.limits(BatchLimits {
            max_offset: 7,
            ..BatchLimits::default()
        });
        batch.query("r", Query::from("users").offset(8));
        assert_eq!(
            run(&batch.build(), &resolver, ExecPolicy::EMBEDDED, mode)
                .await
                .unwrap_err(),
            BatchError::LimitExceeded {
                kind: LimitKind::Offset,
                actual: 8,
                max: 7,
            },
        );
    }
}

// Red before W3: a lowered body limit is ignored, including in an empty loop.
#[tokio::test]
async fn limit_enforcement_nested_pagination_respects_lower_body_caps() {
    let resolver = setup().await;
    for mode in 0..3 {
        for empty_loop in [false, true] {
            let mut body = Batch::new();
            body.limits(BatchLimits {
                max_page_size: 3,
                ..BatchLimits::default()
            });
            body.query("r", Query::from("users").limit(4));
            let mut batch = Batch::new();
            if empty_loop {
                batch.for_each("loop", FilterValue::Array(Vec::new()), "row", body.build());
            } else {
                batch.sub_batch_no_bind("nested", body.build());
            }
            assert_eq!(
                run(&batch.build(), &resolver, ExecPolicy::EMBEDDED, mode)
                    .await
                    .unwrap_err(),
                BatchError::LimitExceeded {
                    kind: LimitKind::PageSize,
                    actual: 4,
                    max: 3,
                },
            );
        }
    }
}

#[tokio::test]
async fn limit_enforcement_defaults_accept_existing_pagination_maxima() {
    let resolver = setup().await;
    let mut batch = Batch::new();
    batch.query("r", Query::from("users").limit(100_000).offset(1_000_000));
    for mode in 0..3 {
        let response = run(&batch.build(), &resolver, ExecPolicy::EMBEDDED, mode)
            .await
            .unwrap();
        assert_eq!(response.results.len(), 1);
        assert!(response.results["r"].records.is_empty());
    }
}

// A permissive server policy still cannot lift any hard engine ceiling.
#[test]
fn limit_enforcement_root_clamps_every_field_to_ceiling() {
    let unlimited = BatchLimits {
        max_queries: usize::MAX,
        max_dependency_depth: usize::MAX,
        max_execution_time_secs: u64::MAX,
        max_result_size: usize::MAX,
        max_nesting_depth: usize::MAX,
        max_iterations: usize::MAX,
        max_total_queries: usize::MAX,
        max_total_work: u64::MAX,
        max_page_size: u64::MAX,
        max_offset: u64::MAX,
    };
    let request = reads(0, unlimited.clone());
    for caps in [unlimited, BatchLimits::CEILING] {
        let auth = Authorized::unchecked(&request, Actor::System, "test").with_policy(ExecPolicy {
            caps,
            enforce_result_size: false,
        });
        assert_eq!(auth.root_scope().limits(), &BatchLimits::CEILING);
    }
}
