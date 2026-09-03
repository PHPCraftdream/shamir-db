//! Group 12 (2026-08-14 cross-crate rush review) — filter-depth and
//! pattern-length/validity DoS guards.
//!
//! Two related defects:
//!
//! 1. **Incomplete filter-depth coverage → process-abort DoS.**
//!    `validate_filter_depth` only collected filters from `Read.where`,
//!    `Delete.where_clause`, `Update.where_clause` — `entry.when` (compiled
//!    unchecked in `query_runner::resolve_skip`) and `GroupBy::having`
//!    (compiled unchecked in `query::read::aggregate`) sailed through with
//!    NO depth check at all. Worse, `check_filter_depth` itself only walked
//!    `And`/`Or`/`Not`, so a filter shallow at that level (e.g. a depth-1
//!    `Eq`) could smuggle unbounded nesting through its embedded VALUE
//!    (`$cond`/`Array` chains) straight past the guard, then blow the stack
//!    inside `compile_filter`/`resolve_filter_query` at execution time — a
//!    process abort, not a catchable `Err`.
//! 2. **Invalid pattern silently becomes "match everything".**
//!    `compile_filter`'s `Regex`/`Like`/`ILike` arms had no pattern-length
//!    cap and folded an invalid pattern to `FilterNode::False` — under a
//!    `NOT` wrapper (common in generated queries) this becomes effectively
//!    `True`, so `DELETE ... WHERE NOT (regex-with-a-typo)` deleted every
//!    row with no error surfaced.

use futures::StreamExt;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;
use shamir_query_builder::write::{self, doc};
use shamir_query_types::filter::{Filter, FilterValue, MAX_FILTER_DEPTH};
use shamir_types::access::Actor;

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::{execute_batch_unchecked as execute_batch, BatchError, TableResolver};
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
        tables: vec![TableConfig::new("users")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    TestResolver { db }
}

async fn count_rows(resolver: &TestResolver, table: &str) -> usize {
    let tbl = resolver.db.get_table("default", table).await.unwrap();
    let stream = tbl.list_stream(64);
    futures::pin_mut!(stream);
    let mut count = 0usize;
    while let Some(batch) = stream.next().await {
        count += batch.unwrap().len();
    }
    count
}

/// Build a `Filter::Not` chain `depth` levels deep, wrapping a trivial
/// `IsNotNull` leaf — cheapest way to exceed `MAX_FILTER_DEPTH` via
/// `Filter`-level (`And`/`Or`/`Not`) nesting.
fn deeply_nested_not_filter(depth: usize) -> Filter {
    let mut f = Filter::IsNotNull {
        field: vec!["x".to_string()],
    };
    for _ in 0..depth {
        f = Filter::Not {
            filter: Box::new(f),
        };
    }
    f
}

/// Build a Filter-level-SHALLOW `Eq` (depth 1) whose VALUE is nested
/// `depth` levels deep via `FilterValue::Array` — the "shallow Filter, deep
/// FilterValue" smuggling shape defect 1 closes.
fn shallow_filter_with_deep_value(depth: usize) -> Filter {
    let mut value = FilterValue::Int(0);
    for _ in 0..depth {
        value = FilterValue::Array(vec![value]);
    }
    Filter::Eq {
        field: vec!["id".to_string()],
        value,
    }
}

fn assert_query_error(err: BatchError, context: &str) {
    assert!(
        matches!(err, BatchError::QueryError { .. }),
        "{context}: expected a QueryError, got {:?}",
        err
    );
}

// ============================================================================
// Defect 1 — regression guard: the EXISTING Read.where / Delete.where_clause
// / Update.where_clause depth check (Filter-level And/Or/Not nesting) must
// still reject correctly after touching the shared collector.
// ============================================================================

#[tokio::test]
async fn read_where_deep_filter_still_rejected() {
    let resolver = setup().await;
    let mut b = Batch::new();
    b.query(
        "r",
        Query::from("users").where_(deeply_nested_not_filter(100)),
    );
    let req = b.build();

    let err = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .expect_err("a 100-level-deep Read.where must still be rejected");
    assert_query_error(err, "Read.where");
}

#[tokio::test]
async fn delete_where_clause_deep_filter_still_rejected() {
    let resolver = setup().await;
    let mut b = Batch::new();
    b.delete(
        "d",
        write::delete("users")
            .where_(deeply_nested_not_filter(100))
            .build()
            .unwrap(),
    );
    let req = b.build();

    let err = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .expect_err("a 100-level-deep Delete.where_clause must still be rejected");
    assert_query_error(err, "Delete.where_clause");
}

#[tokio::test]
async fn update_where_clause_deep_filter_still_rejected() {
    let resolver = setup().await;
    let mut b = Batch::new();
    b.update(
        "u",
        write::update("users")
            .set(doc().set("v", 1_i64))
            .where_(deeply_nested_not_filter(100))
            .build()
            .unwrap(),
    );
    let req = b.build();

    let err = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .expect_err("a 100-level-deep Update.where_clause must still be rejected");
    assert_query_error(err, "Update.where_clause");
}

// ============================================================================
// Defect 1 — NEW coverage: `entry.when` and `GroupBy::having`, plus the
// "shallow Filter, deep FilterValue" smuggling shape.
// ============================================================================

/// `entry.when` was compiled unchecked (`query_runner::resolve_skip`) with
/// NO depth guard at all — a `when` this deep used to sail straight through
/// `validate_filter_depth` into `compile_filter`.
#[tokio::test]
async fn when_guard_with_deep_filter_is_rejected_at_validate_time() {
    let resolver = setup().await;
    let mut b = Batch::new();
    let ins = b.insert("ins", write::insert("users").row(doc().set("name", "x")));
    b.when(&ins, deeply_nested_not_filter(100));
    let req = b.build();

    let err = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .expect_err("a 100-level-deep `when` guard must be rejected before execution");
    assert_query_error(err, "entry.when");
}

/// `GroupBy::having` was compiled unchecked (`query::read::aggregate`) with
/// NO depth guard at all.
#[tokio::test]
async fn having_with_deep_filter_is_rejected_at_validate_time() {
    let resolver = setup().await;
    let mut b = Batch::new();
    b.query(
        "r",
        Query::from("users").having(deeply_nested_not_filter(100)),
    );
    let req = b.build();

    let err = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .expect_err("a 100-level-deep HAVING clause must be rejected before execution");
    assert_query_error(err, "GroupBy.having");
}

/// THE core defect-1 shape: a `Read.where` that is shallow at the
/// `Filter`/`And`/`Or`/`Not` level (a single `Eq`, depth 1) but whose VALUE
/// is nested far past `MAX_FILTER_DEPTH` via `FilterValue::Array` — before
/// the fix, `check_filter_depth` walked only `And`/`Or`/`Not` and never
/// looked at `Eq`'s embedded `value`, so this sailed straight through
/// `validate_filter_depth` untouched and would recurse unbounded inside
/// `resolve_filter_query` at execution time (stack overflow / process
/// abort on a query-reachable path).
#[tokio::test]
async fn where_with_shallow_filter_deep_value_is_rejected_at_validate_time() {
    let resolver = setup().await;
    let deep = shallow_filter_with_deep_value(MAX_FILTER_DEPTH + 10);
    let mut b = Batch::new();
    b.query("r", Query::from("users").where_(deep));
    let req = b.build();

    let err = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .expect_err(
            "an Eq filter with depth 1 at the Filter level, but a VALUE \
             nested far past MAX_FILTER_DEPTH, must be rejected — not left \
             to overflow the stack at execution time",
        );
    assert_query_error(err, "shallow-filter/deep-value smuggling");
}

// ============================================================================
// Defect 2 — pattern length cap.
// ============================================================================

#[tokio::test]
async fn regex_pattern_exceeding_length_cap_is_rejected() {
    let resolver = setup().await;
    let huge_pattern = "a".repeat(pattern_length_cap() + 1);
    let filter = Filter::Regex {
        field: vec!["name".to_string()],
        pattern: huge_pattern,
    };
    let mut b = Batch::new();
    b.query("r", Query::from("users").where_(filter));
    let req = b.build();

    let err = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .expect_err("a regex pattern one byte past the length cap must be rejected");
    assert_query_error(err, "regex pattern length cap");
}

#[tokio::test]
async fn like_pattern_exceeding_length_cap_is_rejected() {
    let resolver = setup().await;
    let huge_pattern = "a".repeat(pattern_length_cap() + 1);
    let filter = Filter::Like {
        field: vec!["name".to_string()],
        pattern: huge_pattern,
    };
    let mut b = Batch::new();
    b.query("r", Query::from("users").where_(filter));
    let req = b.build();

    let err = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .expect_err("a LIKE pattern one byte past the length cap must be rejected");
    assert_query_error(err, "LIKE pattern length cap");
}

/// Mirrors `crate::query::filter::MAX_FILTER_PATTERN_LENGTH` without an
/// engine-internal import path in the module header (kept local since it's
/// only needed to compute "one byte past the cap").
fn pattern_length_cap() -> usize {
    crate::query::filter::MAX_FILTER_PATTERN_LENGTH
}

// ============================================================================
// Defect 2 — invalid pattern must be a hard error, never a silent
// "match-everything" fold.
// ============================================================================

/// THE core defect-2 scenario: `DELETE ... WHERE NOT (regex-with-a-typo)`
/// must be REJECTED with a coded error — before the fix, `compile_filter`
/// folded the unparseable regex to `FilterNode::False`, which `Not` then
/// turned into effectively `True`, deleting every row with no error
/// surfaced to the caller.
#[tokio::test]
async fn delete_with_invalid_regex_under_not_is_rejected_not_delete_all() {
    let resolver = setup().await;

    // Seed a few rows so a silent "delete everything" would be observable.
    let mut seed = Batch::new();
    for i in 0..3 {
        seed.insert(
            format!("seed{i}"),
            write::insert("users").row(doc().set("name", format!("user{i}"))),
        );
    }
    execute_batch(&seed.build(), &resolver, None, None, Actor::System, "test")
        .await
        .expect("seeding must succeed");
    assert_eq!(count_rows(&resolver, "users").await, 3);

    // Unbalanced parenthesis — invalid regex syntax.
    let invalid_regex = Filter::Regex {
        field: vec!["name".to_string()],
        pattern: "(unbalanced".to_string(),
    };
    let where_clause = Filter::Not {
        filter: Box::new(invalid_regex),
    };
    let mut b = Batch::new();
    b.delete(
        "d",
        write::delete("users").where_(where_clause).build().unwrap(),
    );
    let req = b.build();

    let err = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .expect_err(
            "DELETE ... WHERE NOT (invalid regex) must error, not silently \
             compile to a full-table match",
        );
    assert_query_error(err, "invalid regex under NOT");

    // Decisive: nothing was deleted.
    assert_eq!(
        count_rows(&resolver, "users").await,
        3,
        "an invalid regex under NOT must not delete any row"
    );
}

/// Same mechanism, `Filter::Like` — `like_pattern_to_regex` escapes every
/// regex metacharacter so a LIKE pattern can only fail to compile for a
/// resource-limit reason, not a syntax one; this is the length-cap arm's
/// paired proof that an over-cap LIKE pattern is rejected as invalid, not
/// silently folded to `False`/`True`.
#[tokio::test]
async fn delete_with_overlong_like_under_not_is_rejected_not_delete_all() {
    let resolver = setup().await;

    let mut seed = Batch::new();
    seed.insert(
        "seed",
        write::insert("users").row(doc().set("name", "alice")),
    );
    execute_batch(&seed.build(), &resolver, None, None, Actor::System, "test")
        .await
        .expect("seeding must succeed");
    assert_eq!(count_rows(&resolver, "users").await, 1);

    let overlong = Filter::Like {
        field: vec!["name".to_string()],
        pattern: "a".repeat(pattern_length_cap() + 1),
    };
    let where_clause = Filter::Not {
        filter: Box::new(overlong),
    };
    let mut b = Batch::new();
    b.delete(
        "d",
        write::delete("users").where_(where_clause).build().unwrap(),
    );
    let req = b.build();

    let err = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .expect_err("DELETE ... WHERE NOT (overlong LIKE) must error");
    assert_query_error(err, "overlong LIKE under NOT");

    assert_eq!(
        count_rows(&resolver, "users").await,
        1,
        "an overlong LIKE pattern under NOT must not delete any row"
    );
}
