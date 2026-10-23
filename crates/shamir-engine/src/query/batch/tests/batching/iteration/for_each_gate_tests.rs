use shamir_query_types::batch::{
    BatchLimits, BatchOp, BatchRequest, ForEachOp, QueryEntry, ResultEncoding,
};
use shamir_query_types::filter::FilterValue;
use shamir_query_types::write::InsertOp;
use shamir_types::access::Actor;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use super::support::{setup, wrap_for_each};
use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::{execute_batch_unchecked as execute_batch, TableResolver};
use crate::query::TableRef;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoInstance;
use crate::table::{TableConfig, TableManager};
use shamir_storage::error::DbResult;

/// Build a `for_each` body with TWO Insert ops touching TWO different
/// tables (`orders` and `users`) -- both bound to the SAME `$param`
/// (`bind_row`). Used to prove multi-query body correctness AND to give
/// the call-count proof below two distinct tables to count
/// (`validate_tables` dedups per table, so a 2-table body makes the "ran
/// once, not N times" arithmetic unambiguous).
fn insert_order_and_user_body(bind_row: &str) -> BatchRequest {
    let mut param_obj = new_map();
    param_obj.insert("$param".to_string(), QueryValue::Str(bind_row.to_string()));

    let mut order_value = new_map();
    order_value.insert("user_id".to_string(), QueryValue::Map(param_obj.clone()));
    order_value.insert("note".to_string(), QueryValue::Str("fe".to_string()));

    let mut user_value = new_map();
    user_value.insert("uid".to_string(), QueryValue::Map(param_obj));
    user_value.insert("name".to_string(), QueryValue::Str("fe_user".to_string()));

    let mut queries = new_map();
    queries.insert(
        "ins_order".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: TableRef::new("orders"),
                values: vec![QueryValue::Map(order_value)],
                records_idmsgpack: Vec::new(),
                select: None,
            }),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    queries.insert(
        "ins_user".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: TableRef::new("users"),
                values: vec![QueryValue::Map(user_value)],
                records_idmsgpack: Vec::new(),
                select: None,
            }),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    BatchRequest {
        id: QueryValue::Int(103),
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

/// Wraps a [`TestResolver`]'s `db` and counts every `resolve()` call --
/// used to prove `validate_tables` (invoked once via
/// `plan_and_validate_batch`, hoisted out of the `ForEach` loop) is no
/// longer invoked once per iteration.
struct CountingResolver {
    db: DbInstance,
    resolve_calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait::async_trait]
impl TableResolver for CountingResolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        self.resolve_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.db.get_table("default", &table_ref.table).await
    }

    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<RepoInstance> {
        self.db.get_repo("default").ok_or_else(|| {
            shamir_storage::error::DbError::NotFound("repo 'default' not found".into())
        })
    }
}

/// Same counting wrapper as [`CountingResolver`], but over a
/// [`TxTestResolver`]-shaped `RepoInstance` -- used to prove the fix on the
/// tx arm (`run_nested_body_in_outer_tx`), not just the non-tx arm.
struct CountingTxResolver {
    repo: RepoInstance,
    resolve_calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait::async_trait]
impl TableResolver for CountingTxResolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        self.resolve_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.repo.get_table(&table_ref.table).await
    }

    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<RepoInstance> {
        Ok(self.repo.clone())
    }
}

/// (b) A `for_each` body referencing a table that does not exist must still
/// fail validation -- and it must do so BEFORE iteration 0 ever dispatches
/// any op, not mid-loop after some iterations already ran (proven by
/// asserting zero rows in EITHER table the body touches, across an `over`
/// with 3 elements).
#[tokio::test]
async fn for_each_body_table_not_found_fails_before_first_iteration_non_tx() {
    let resolver = setup().await;

    let mut bad_body = insert_order_and_user_body("uid");
    if let Some(entry) = bad_body.queries.get_mut("ins_order") {
        if let BatchOp::Insert(op) = &mut entry.op {
            op.insert_into = TableRef::new("nonexistent_table");
        }
    }

    let fe = ForEachOp {
        over: FilterValue::Array(vec![
            FilterValue::Int(1),
            FilterValue::Int(2),
            FilterValue::Int(3),
        ]),
        bind_row: "uid".to_string(),
        batch: bad_body,
    };
    let req = wrap_for_each(fe);

    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .expect_err("a for_each body referencing a nonexistent table must fail validation");

    // Zero rows in `users` -- proves iteration 0 never even ran its FIRST
    // op (`ins_order` runs before `ins_user` isn't guaranteed by stage
    // order, but neither alias has an `after` edge onto the other, so both
    // are in the SAME stage -- what matters is validation rejected the
    // whole body before either op of any iteration dispatched).
    let mut read_queries = new_map();
    read_queries.insert(
        "u".to_string(),
        QueryEntry {
            op: BatchOp::Read(crate::query::read::ReadQuery::from(
                shamir_query_builder::query::Query::from("users"),
            )),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    let read_req = BatchRequest {
        id: QueryValue::Int(10),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
        queries: read_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };
    let read_resp = execute_batch(&read_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert_eq!(
        read_resp.results["u"].records.len(),
        0,
        "no iteration must have written anything -- validation must reject \
         the body before the loop starts iterating, not mid-loop"
    );
}

/// (a) + (c): a multi-query `for_each` body (2 Insert ops touching 2
/// different tables) over N iterations still produces byte-identical,
/// fully correct results after the hoist (N rows in EACH table) -- AND a
/// `resolve()` call counter proves `validate_tables` ran exactly ONCE for
/// the whole loop, not once per iteration.
#[tokio::test]
async fn for_each_plan_validate_runs_once_not_per_iteration_non_tx() {
    let resolver = setup().await;
    let resolve_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counting = CountingResolver {
        db: resolver.db.clone(),
        resolve_calls: resolve_calls.clone(),
    };

    let n = 5usize;
    let fe = ForEachOp {
        over: FilterValue::Array((0..n as i64).map(FilterValue::Int).collect()),
        bind_row: "uid".to_string(),
        batch: insert_order_and_user_body("uid"),
    };
    let req = wrap_for_each(fe);

    let resp = execute_batch(&req, &counting, None, None, Actor::System, "test")
        .await
        .unwrap();
    let list = resp.results["loop"]
        .value
        .as_ref()
        .unwrap()
        .as_array()
        .expect("loop value must be a List");
    assert_eq!(list.len(), n, "n elements -> n iterations");

    // Decisive call-count proof, captured BEFORE the read-back verification
    // below adds its own resolve() calls.
    //
    // `plan_and_validate_batch` (`BatchPlanner::plan` + `validate_tables`)
    // now runs ONCE for the whole loop, resolving the body's 2 distinct
    // tables exactly once each ("validate" below).
    //
    // Each of the n iterations' Insert dispatches then resolves its OWN
    // table once more ("dispatch" below) -- `QueryRunner::run`'s per-op
    // resolve, unrelated to this fix, unavoidable real work needed to get
    // the actual `TableManager` handle.
    //
    // PLUS a one-time, constant overhead unrelated to this fix or to `n`:
    // the very FIRST Insert's `require_footprint_if_fk_child` hook warms
    // the repo's FK-reverse-cache (`get_or_build_by_parent` ->
    // `build_reverse_fk_entries`), which does its own repo-wide
    // `resolver.resolve()` scan over EVERY table in the repo (2 here) --
    // see `fk_reverse_cache.rs`. That scan runs exactly ONCE ever (the
    // cache is warm for every subsequent Insert, in this iteration and
    // all future ones), so it does NOT scale with `n` and is orthogonal
    // to the P2 fix under test ("fk_warmup" below).
    //
    // Total = validate (2) + fk_warmup (2) + dispatch (n*2). Before the P2
    // fix, `validate_tables` re-ran INSIDE the loop on every iteration
    // too (fk_warmup is unaffected either way -- it was already a
    // once-only cache build before this fix), so the total would have
    // been n*2 (validate) + 2 (fk_warmup) + n*2 (dispatch) = 4*n + 2.
    let calls = resolve_calls.load(std::sync::atomic::Ordering::SeqCst);
    let validate = 2;
    let fk_warmup = 2;
    let dispatch = n * 2;
    let expected = validate + fk_warmup + dispatch;
    let pre_fix_would_be = n * 2 + fk_warmup + dispatch;
    assert_eq!(
        calls, expected,
        "expected 1 upfront validate_tables pass ({validate} resolves, 2 \
         distinct tables) + {fk_warmup} one-time FK-reverse-cache warmup \
         resolves (once ever, not per-iteration) + {dispatch} \
         per-iteration dispatch resolves (2 tables x {n} iterations) = \
         {expected}; got {calls} resolve() calls -- {pre_fix_would_be} \
         would mean validate_tables is still re-running once per \
         iteration (the bug this fix closes)"
    );

    // Correctness: the multi-query body ran to completion on EVERY
    // iteration (not just validated once and then silently skipped) --
    // exactly n rows landed in BOTH tables.
    let mut read_queries = new_map();
    read_queries.insert(
        "orders".to_string(),
        QueryEntry {
            op: BatchOp::Read(crate::query::read::ReadQuery::from(
                shamir_query_builder::query::Query::from("orders"),
            )),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    read_queries.insert(
        "users".to_string(),
        QueryEntry {
            op: BatchOp::Read(crate::query::read::ReadQuery::from(
                shamir_query_builder::query::Query::from("users"),
            )),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    let read_req = BatchRequest {
        id: QueryValue::Int(11),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
        queries: read_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };
    let read_resp = execute_batch(&read_req, &counting, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert_eq!(read_resp.results["orders"].records.len(), n);
    assert_eq!(read_resp.results["users"].records.len(), n);
}

/// Same call-count proof as
/// [`for_each_plan_validate_runs_once_not_per_iteration_non_tx`], but for
/// the TX arm (`run_nested_body_in_outer_tx`) -- an outer `transactional:
/// true` batch containing the `for_each`, so every iteration reuses the
/// outer `TxContext` (#661).
#[tokio::test]
async fn for_each_plan_validate_runs_once_not_per_iteration_tx() {
    let factory = BoxRepoFactory::in_memory();
    let repo = RepoInstance::from_factory(
        "test".into(),
        factory,
        vec![TableConfig::new("orders"), TableConfig::new("users")],
    )
    .await
    .unwrap();
    let resolve_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counting = CountingTxResolver {
        repo: repo.clone(),
        resolve_calls: resolve_calls.clone(),
    };

    let n = 4usize;
    let fe_entry = QueryEntry {
        op: BatchOp::ForEach(ForEachOp {
            over: FilterValue::Array((0..n as i64).map(FilterValue::Int).collect()),
            bind_row: "uid".to_string(),
            batch: insert_order_and_user_body("uid"),
        }),
        return_result: true,
        after: Vec::new(),
        when: None,
    };
    let mut outer_queries = new_map();
    outer_queries.insert("loop".to_string(), fe_entry);
    let outer_req = BatchRequest {
        id: QueryValue::Int(12),
        name: None,
        transactional: true,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
        queries: outer_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    let resp = execute_batch(&outer_req, &counting, None, None, Actor::System, "test")
        .await
        .unwrap();
    let tx_info = resp
        .transaction
        .as_ref()
        .expect("transactional batch must carry TransactionInfo");
    assert_eq!(
        tx_info.status, "committed",
        "all n iterations must succeed and the outer tx must commit"
    );

    // Same accounting as
    // `for_each_plan_validate_runs_once_not_per_iteration_non_tx`: validate
    // (2, once) + fk_warmup (2, one-time repo-wide FK-reverse-cache scan on
    // the very first Insert, unrelated to this fix and not per-iteration)
    // + dispatch (n*2, each iteration's 2 real Insert resolves).
    let calls = resolve_calls.load(std::sync::atomic::Ordering::SeqCst);
    let validate = 2;
    let fk_warmup = 2;
    let dispatch = n * 2;
    let expected = validate + fk_warmup + dispatch;
    let pre_fix_would_be = n * 2 + fk_warmup + dispatch;
    assert_eq!(
        calls, expected,
        "run_nested_body_in_outer_tx must plan/validate the loop body ONCE \
         before iterating (1 pass x 2 distinct tables) + {fk_warmup} \
         one-time FK-reverse-cache warmup resolves + {dispatch} \
         per-iteration dispatch resolves; got {calls} resolve() calls, \
         expected {expected} -- {pre_fix_would_be} would mean the pre-fix \
         once-per-iteration re-validation is back"
    );
}
