//! W6 runtime-budget regressions. Expected units are derived from dispatches,
//! not from the implementation's visitor or counter. No clocks or sleeps.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::val::param;
use shamir_query_builder::write::{doc, insert};
use shamir_query_builder::Query;
use shamir_query_types::batch::{BatchLimits, BatchRequest, LimitKind, ResultEncoding};
use shamir_query_types::filter::Filter;
use shamir_types::access::Actor;
use shamir_types::types::common::new_map;

use crate::query::batch::batch_execute::{
    execute_batch_impl, execute_plan_impl, execute_plan_tx, execute_transactional_impl,
};
use crate::query::batch::{
    commit_interactive_tx, execute_batch_unchecked, open_interactive_tx, BatchError, ExecScope,
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
    let resolver = Resolver {
        repo: RepoInstance::from_factory(
            "main".into(),
            BoxRepoFactory::in_memory(),
            vec![TableConfig::new("source"), TableConfig::new("sink")],
        )
        .await
        .unwrap(),
    };
    let mut seed = Batch::new();
    seed.insert(
        "seed",
        insert("source").rows((0..10).map(|id| doc().set("id", id))),
    );
    execute_batch_unchecked(&seed.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert_eq!(rows(&resolver, "source").await, 10);
    resolver
}

async fn rows(resolver: &Resolver, table: &str) -> usize {
    let mut batch = Batch::new();
    batch.query("rows", Query::from(table).limit(1000));
    execute_batch_unchecked(&batch.build(), resolver, None, None, Actor::System, "test")
        .await
        .unwrap()
        .results["rows"]
        .records
        .len()
}

fn nested(cap: u64, transactional: bool) -> BatchRequest {
    let mut leaf = Batch::new();
    leaf.insert("write", insert("sink").row(doc().set("id", param("inner"))));
    let mut body = Batch::new();
    let src = body.query("src", Query::from("source"));
    body.for_each("inner", src.column("id"), "inner", leaf.build());
    let mut batch = Batch::new();
    batch.return_all().limits(BatchLimits {
        max_total_work: cap,
        ..BatchLimits::default()
    });
    if transactional {
        batch.transactional();
    }
    let src = batch.query("src", Query::from("source"));
    batch.for_each("outer", src.column("id"), "outer", body.build());
    batch.build()
}

async fn run(
    request: &BatchRequest,
    resolver: &Resolver,
    scope: &ExecScope,
) -> Result<(), BatchError> {
    let response = execute_batch_impl(
        request,
        resolver,
        None,
        None,
        Actor::System,
        "test",
        0,
        &new_map(),
        scope,
    )
    .await?;
    assert_eq!(response.results["src"].records.len(), 10);
    assert!(response.results["outer"].value.is_some());
    if request.transactional {
        assert!(response.transaction.unwrap().is_committed());
    }
    Ok(())
}

// Red before W6: dynamic loops escape the static gate, run all 100 writes,
// and leave the shared counter at zero instead of rejecting dispatch 51.
#[tokio::test]
async fn dynamic_nested_work_limit_and_transactional_rollback() {
    for transactional in [false, true] {
        let resolver = setup().await;
        let request = nested(50, transactional);
        let scope = ExecScope::root(&request.limits);
        assert_eq!(
            run(&request, &resolver, &scope).await.unwrap_err(),
            work_error(51, 50),
        );
        assert_eq!(scope.work(), 51);
        assert_eq!(
            rows(&resolver, "sink").await,
            if transactional { 0 } else { 40 },
        );
        assert_eq!(rows(&resolver, "source").await, 10);
    }
}

// Red before W6: observable count is zero. Double charging the precheck,
// omitting either stage loop, or charging commit also violates this oracle.
#[tokio::test]
async fn dynamic_nested_generous_and_exact_caps_count_122() {
    // 1 root read + 1 outer loop + 10 * (1 body read + 1 inner loop + 10 writes).
    for cap in [1000, 122] {
        for transactional in [false, true] {
            let resolver = setup().await;
            let request = nested(cap, transactional);
            let scope = ExecScope::root(&request.limits);
            run(&request, &resolver, &scope).await.unwrap();
            assert_eq!(scope.work(), 122);
            assert_eq!(rows(&resolver, "sink").await, 100);
        }
    }
}

// Red before W6: iteration zero commits a side effect. Counting only the
// two immediate body queries checks 2 + 10 * 2 = 22 and wrongly admits it.
// The whole body tree checks 2 + 10 * 3 = 32 before any iteration writes.
// W5 sees only five static template entries, below the cap of 25.
#[tokio::test]
async fn dynamic_precheck_rejects_before_iteration_zero_without_charging() {
    let resolver = setup().await;
    let mut nested_body = Batch::new();
    nested_body.query("read", Query::from("source"));
    let mut body = Batch::new();
    let write = body.insert("write", insert("sink").row(doc().set("id", param("row"))));
    let nested = body.sub_batch_no_bind("nested", nested_body.build());
    body.after(&nested, &write);
    let mut batch = Batch::new();
    batch.limits(BatchLimits {
        max_total_work: 25,
        ..BatchLimits::default()
    });
    let src = batch.query("src", Query::from("source"));
    batch.for_each("loop", src.column("id"), "row", body.build());
    let request = batch.build();
    let scope = ExecScope::root(&request.limits);
    assert_eq!(
        execute_batch_impl(
            &request,
            &resolver,
            None,
            None,
            Actor::System,
            "test",
            0,
            &new_map(),
            &scope,
        )
        .await
        .unwrap_err(),
        work_error(32, 25),
    );
    assert_eq!(scope.work(), 2);
    assert_eq!(rows(&resolver, "sink").await, 0);
}

fn work_error(actual: u64, max: u64) -> BatchError {
    BatchError::LimitExceeded {
        kind: LimitKind::Work,
        actual,
        max,
    }
}

#[test]
fn work_checks_do_not_charge_and_reject_overflow() {
    let scope = ExecScope::root(&BatchLimits {
        max_total_work: 7,
        ..BatchLimits::default()
    });
    scope.charge_work(3).unwrap();
    scope.check_work(4).unwrap();
    assert_eq!(scope.work(), 3);
    assert_eq!(scope.check_work(5).unwrap_err(), work_error(8, 7));
    assert_eq!(
        scope.check_work(u64::MAX).unwrap_err(),
        work_error(u64::MAX, 7)
    );
    scope.charge_work(4).unwrap();
    scope.check_work(0).unwrap();
    assert_eq!(scope.charge_work(1).unwrap_err(), work_error(8, 7));
    assert_eq!(scope.work(), 8);
    assert_eq!(scope.check_work(0).unwrap_err(), work_error(8, 7));
}

// Removing the precommit check alone turns this into a committed response.
// The plan is deliberately empty after planning a repo-targeting request:
// this isolates the final check from the dispatch-time checks.
#[tokio::test]
async fn exhausted_scope_is_rejected_before_closed_transaction_commit() {
    let resolver = setup().await;
    let mut batch = Batch::new();
    batch.transactional().limits(BatchLimits {
        max_total_work: 1,
        ..BatchLimits::default()
    });
    batch.query("src", Query::from("source"));
    let request = batch.build();
    let scope = ExecScope::root(&request.limits);
    assert_eq!(scope.charge_work(2).unwrap_err(), work_error(2, 1));
    let mut plan =
        shamir_query_types::batch::BatchPlanner::plan(&request.queries, &request.limits).unwrap();
    plan.stages.clear();
    let version = resolver.repo.current_commit_version().await.unwrap();
    assert_eq!(
        execute_transactional_impl(
            &request,
            &mut plan,
            &resolver,
            None,
            None,
            &Actor::System,
            "test",
            0,
            &new_map(),
            ResultEncoding::Name,
            &scope,
        )
        .await
        .unwrap_err(),
        work_error(2, 1),
    );
    assert_eq!(
        resolver.repo.current_commit_version().await.unwrap(),
        version
    );
    assert_eq!(scope.work(), 2);
}

// Bypass only the conservative static gate to isolate actual dispatch counts.
// Charging before resolve_skip would consume all three units, not one.
#[tokio::test]
async fn skipped_entries_are_not_charged_in_either_stage_loop() {
    let resolver = setup().await;
    let mut batch = Batch::new();
    batch.return_all();
    let skipped = batch.query("skipped", Query::from("source"));
    batch.when(
        &skipped,
        Filter::Or {
            filters: Vec::new(),
        },
    );
    let mut body = Batch::new();
    body.query("read", Query::from("source"));
    batch.for_each("cascade", skipped.column("id"), "row", body.build());
    batch.query("live", Query::from("source"));
    let request = batch.build();
    for transactional in [false, true] {
        let scope = ExecScope::root(&BatchLimits {
            max_total_work: 1,
            ..BatchLimits::default()
        });
        let mut plan =
            shamir_query_types::batch::BatchPlanner::plan(&request.queries, &request.limits)
                .unwrap();
        let results = if transactional {
            let (mut tx, guard) =
                open_interactive_tx(&resolver.repo, shamir_tx::IsolationLevel::Snapshot)
                    .await
                    .unwrap();
            let results = execute_plan_tx(
                &mut plan,
                &request.queries,
                &resolver,
                None,
                None,
                &Actor::System,
                "test",
                &mut tx,
                &scope,
            )
            .await
            .unwrap();
            drop(tx);
            drop(guard);
            results
        } else {
            execute_plan_impl(
                &mut plan,
                &request.queries,
                &resolver,
                None,
                None,
                &Actor::System,
                "test",
                0,
                &new_map(),
                ResultEncoding::Name,
                &scope,
            )
            .await
            .unwrap()
        };
        assert!(results["skipped"].skipped);
        assert!(results["cascade"].skipped);
        assert_eq!(results["live"].records.len(), 10);
        assert_eq!(scope.work(), 1);
    }
}

#[test]
fn work_overflow_cannot_wrap_into_a_successful_budget_check() {
    let scope = ExecScope::root(&BatchLimits {
        max_total_work: u64::MAX,
        ..BatchLimits::default()
    });
    scope.charge_work(u64::MAX).unwrap();
    assert_eq!(
        scope.check_work(1).unwrap_err(),
        work_error(u64::MAX, u64::MAX)
    );
    assert_eq!(
        scope.charge_work(1).unwrap_err(),
        work_error(u64::MAX, u64::MAX)
    );
    assert_eq!(scope.work(), u64::MAX);
    assert_eq!(
        scope.check_work(1).unwrap_err(),
        work_error(u64::MAX, u64::MAX)
    );
}

#[tokio::test]
async fn interactive_commit_does_not_charge_or_check_execution_budget() {
    let resolver = setup().await;
    let mut batch = Batch::new();
    batch.limits(BatchLimits {
        max_total_work: 1,
        ..BatchLimits::default()
    });
    batch.insert("write", insert("sink").row(doc().set("id", 42)));
    let request = batch.build();
    let scope = ExecScope::root(&request.limits);
    let mut plan =
        shamir_query_types::batch::BatchPlanner::plan(&request.queries, &request.limits).unwrap();
    let (mut tx, guard) = open_interactive_tx(&resolver.repo, shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();
    execute_plan_tx(
        &mut plan,
        &request.queries,
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut tx,
        &scope,
    )
    .await
    .unwrap();
    assert_eq!(scope.work(), 1);
    assert_eq!(rows(&resolver, "sink").await, 0);
    // Exhaust the EXECUTE scope after staging. TxCommit is a separate operation
    // and must complete regardless; there is no budget inside its commit future.
    assert_eq!(scope.charge_work(1).unwrap_err(), work_error(2, 1));
    let outcome = commit_interactive_tx(&resolver.repo, tx).await.unwrap();
    drop(guard);
    assert!(outcome.commit_version > 0);
    assert_eq!(scope.work(), 2);
    assert_eq!(rows(&resolver, "sink").await, 1);
}
