//! Tests for `QueryEntry.when` conditional execution + cascade skip
//! (Epic03/B, #645). See `docs/dev-artifacts/design/oql-03-conditional-execution-adr.md`.
//!
//! `Batch`'s fluent `when`/switch-case ergonomics are Epic03/C (#646) — not
//! implemented yet, so these tests set `QueryEntry.when` directly on the
//! built `BatchRequest.queries` map (a public field), after assembling the
//! rest of the batch with the query builder.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_query_types::filter::Filter;
use shamir_types::access::Actor;

use crate::query::batch::execute_batch;

use super::common::setup_resolver;

/// A `when` that evaluates to `true` (against the empty synthetic record)
/// executes the op normally: `skipped: false`, real records.
#[tokio::test]
async fn when_true_executes_op_normally() {
    let resolver = setup_resolver().await;

    let mut b = Batch::new();
    b.id(1);
    b.query("probe", Query::from("users"));
    let mut req = b.build();

    // `Filter::Eq`'s LHS is always a field path (there is no
    // literal-vs-literal `Filter` variant), so the simplest
    // always-`true`-against-the-empty-synthetic-record filter is `IsNull`
    // on a field that can never be present — a synthetic record has no
    // fields at all, so `IsNull` on any path is always `true`.
    req.queries.get_mut("probe").unwrap().when = Some(Filter::IsNull {
        field: vec!["anything".to_string()],
    });

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let result = &resp.results["probe"];
    assert!(
        !result.skipped,
        "when: IsNull(missing field) on the empty synthetic record must \
         evaluate true and execute the op"
    );
}

/// A `when` that evaluates to `false` skips the op: `skipped: true`, empty
/// records/stats/pagination/value/explain.
#[tokio::test]
async fn when_false_skips_op() {
    let resolver = setup_resolver().await;

    // Seed one row so a non-skipped read would return something (to make
    // the skip observable, not just "coincidentally empty").
    let mut seed = Batch::new();
    seed.id(1);
    seed.op_silent(
        "seed",
        write::insert("users").row(doc().set("name", "Alice")),
    );
    let seed_req = seed.build();
    execute_batch(&seed_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(2);
    b.query("probe", Query::from("users"));
    let mut req = b.build();

    // `IsNotNull` on a field that never exists on the empty synthetic
    // record evaluates false.
    req.queries.get_mut("probe").unwrap().when = Some(Filter::IsNotNull {
        field: vec!["anything".to_string()],
    });

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let result = &resp.results["probe"];
    assert!(
        result.skipped,
        "when: IsNotNull(missing field) must evaluate false and skip the op"
    );
    assert!(result.records.is_empty());
    assert!(result.stats.is_none());
    assert!(result.pagination.is_none());
    assert!(result.value.is_none());
    assert!(result.explain.is_none());
}

/// Cascade: `B` depends on `A` via a real `$query` ref (DataFlow edge).
/// `A` is skipped (own `when` false) → `B` is automatically skipped too,
/// without error, even though `B` itself carries no `when`.
#[tokio::test]
async fn cascade_skip_propagates_through_dataflow_edge() {
    let resolver = setup_resolver().await;

    let mut seed = Batch::new();
    seed.id(1);
    seed.op_silent(
        "seed",
        write::insert("users").row(doc().set("name", "Alice").set("status", "active")),
    );
    let seed_req = seed.build();
    execute_batch(&seed_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(2);
    let a = b.query("a", Query::from("users").where_eq("status", "active"));
    // `b_dep` genuinely references `a`'s result via $query (DataFlow edge).
    b.query(
        "b_dep",
        Query::from("users").where_eq("name", a.first().field("name")),
    );
    let mut req = b.build();

    // Force `a` to be skipped.
    req.queries.get_mut("a").unwrap().when = Some(Filter::IsNotNull {
        field: vec!["never".to_string()],
    });

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    assert!(resp.results["a"].skipped, "a's own when must skip it");
    assert!(
        resp.results["b_dep"].skipped,
        "b_dep depends on skipped 'a' via a real $query (DataFlow) ref — \
         it must cascade-skip automatically, not error and not silently \
         see an absent/None value"
    );
}

/// `after`-only (Explicit) dependency on a skipped alias does NOT cascade:
/// the dependent still runs (it has no own `when` and no `DataFlow`/`Both`
/// edge onto the skipped alias) — it simply loses whatever ordering
/// guarantee "after A" was providing.
#[tokio::test]
async fn after_only_dependency_on_skipped_alias_does_not_cascade() {
    let resolver = setup_resolver().await;

    let mut seed = Batch::new();
    seed.id(1);
    seed.op_silent(
        "seed",
        write::insert("users").row(doc().set("name", "Alice").set("status", "active")),
    );
    let seed_req = seed.build();
    execute_batch(&seed_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(2);
    let a = b.query("a", Query::from("users").where_eq("status", "active"));
    // `marker` only orders after `a` (no $query ref on `a` anywhere) —
    // Explicit-only edge.
    let marker = b.op_silent(
        "marker",
        write::insert("users").row(doc().set("name", "Marker").set("status", "marker")),
    );
    b.after(&marker, &a);
    let mut req = b.build();

    // Force `a` to be skipped.
    req.queries.get_mut("a").unwrap().when = Some(Filter::IsNotNull {
        field: vec!["never".to_string()],
    });

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    assert!(resp.results["a"].skipped, "a's own when must skip it");
    assert!(
        !resp.results["marker"].skipped,
        "marker's only edge onto 'a' is Explicit (after-only) — it must \
         NOT cascade-skip; it should run normally even though 'a' was \
         skipped"
    );
    assert!(
        resp.results["marker"].stats.is_some(),
        "marker must have actually executed (real insert stats present)"
    );
}
