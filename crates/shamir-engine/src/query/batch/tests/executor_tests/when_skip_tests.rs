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
use shamir_query_types::filter::{Filter, FilterValue, FnCall, ValueCompareOp};
use shamir_types::access::Actor;

use crate::query::batch::execute_batch;

use super::common::{setup_resolver, setup_resolver_with_scalars, TxTestResolver};

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

/// Gap #1 (brief item 1): a skipped WRITE op (Insert with `when: false`)
/// inside a TRANSACTIONAL batch must not enter the tx write-set at all —
/// the commit must succeed cleanly and an observer reading AFTER commit
/// must see zero rows from the skipped insert (only rows from the
/// unconditional sibling insert that actually ran).
#[tokio::test]
async fn skipped_write_inside_transactional_batch_has_no_side_effect_on_commit() {
    use futures::StreamExt;

    let factory = crate::repo::repo_types::BoxRepoFactory::in_memory();
    let repo = crate::repo::RepoInstance::from_factory(
        "test".into(),
        factory,
        vec![crate::table::TableConfig::new("users")],
    )
    .await
    .unwrap();
    let resolver = TxTestResolver { repo: repo.clone() };

    let mut b = Batch::new();
    b.id(1);
    b.transactional();
    b.op_silent(
        "unconditional",
        write::insert("users").row(doc().set("name", "always")),
    );
    let guarded = b.op_silent(
        "guarded",
        write::insert("users").row(doc().set("name", "never")),
    );
    let mut req = b.build();
    // `IsNotNull` on a field that never exists on the empty synthetic
    // record evaluates false — guards this insert off.
    req.queries.get_mut(guarded.alias()).unwrap().when = Some(Filter::IsNotNull {
        field: vec!["never_present".to_string()],
    });

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let info = resp.transaction.expect("transaction info present");
    assert_eq!(info.status, "committed", "commit must succeed cleanly");
    assert!(
        resp.results["guarded"].skipped,
        "the guarded insert must be marked skipped"
    );

    // Observer reads AFTER commit, outside the tx — only the unconditional
    // row must be visible; the skipped insert must have left no trace in
    // the committed write-set.
    let tbl = repo.get_table("users").await.unwrap();
    let stream = tbl.list_stream(64);
    futures::pin_mut!(stream);
    let mut raw_count = 0usize;
    while let Some(page) = stream.next().await {
        raw_count += page.unwrap().len();
    }
    assert_eq!(
        raw_count, 1,
        "raw table scan must see exactly 1 committed row"
    );

    // Cross-check via a plain query read too, for a second independent view.
    let mut qb = Batch::new();
    qb.id(2);
    qb.query("all", Query::from("users"));
    let qreq = qb.build();
    let qresp = execute_batch(&qreq, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert_eq!(
        qresp.results["all"].records.len(),
        1,
        "only the unconditional insert's row must be committed; the \
         skipped insert must not have entered the tx write-set"
    );
}

/// Gap #2 (brief item 2): `when: false` on an alias whose op is a nested
/// `BatchOp::Batch` (Epic01 sub-batch) must skip the WHOLE recursion — the
/// inner ops of the sub-batch must never execute at all, not just have
/// their own results discarded.
#[tokio::test]
async fn when_false_on_sub_batch_alias_skips_entire_inner_recursion() {
    let resolver = setup_resolver().await;

    // Build the inner batch: a plain insert that would be observable if it
    // ran (a seed check reads back afterwards).
    let mut inner = Batch::new();
    inner.id(99);
    inner.op_silent(
        "inner_insert",
        write::insert("users").row(doc().set("name", "from-sub-batch")),
    );
    let inner_req = inner.build();

    let mut outer = Batch::new();
    outer.id(1);
    let sub = outer.sub_batch_no_bind("sub", inner_req);
    let mut req = outer.build();

    // Force the sub-batch alias to be skipped.
    req.queries.get_mut(sub.alias()).unwrap().when = Some(Filter::IsNotNull {
        field: vec!["never".to_string()],
    });

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    assert!(
        resp.results["sub"].skipped,
        "the sub-batch alias itself must be marked skipped"
    );

    // Confirm the inner insert never ran: a fresh read against "users"
    // must see zero rows.
    let mut qb = Batch::new();
    qb.id(2);
    qb.query("all", Query::from("users"));
    let qreq = qb.build();
    let qresp = execute_batch(&qreq, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert_eq!(
        qresp.results["all"].records.len(),
        0,
        "when: false on a sub-batch alias must skip its ENTIRE inner \
         recursion — the inner insert must never have executed"
    );
}

/// Gap #3 (brief item 3): `B` has BOTH an `after`-edge onto a skipped `A`
/// (Explicit, non-cascading) AND its own independent `when` (unrelated to
/// `A`). `B` must run purely according to its own `when`, regardless of
/// `A` being skipped — the `after`-from-skipped-`A` edge contributes no
/// cascade, only a (now vacuous) ordering hint.
#[tokio::test]
async fn after_edge_from_skipped_alias_combined_with_own_independent_when() {
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
    // `b_op` has an after-edge on `a` (Explicit-only, no $query ref to `a`)
    // AND its own independent `when`.
    let b_op = b.op_silent(
        "b_op",
        write::insert("users").row(doc().set("name", "B").set("status", "b-marker")),
    );
    b.after(&b_op, &a);
    let mut req = b.build();

    // Force `a` to be skipped.
    req.queries.get_mut(a.alias()).unwrap().when = Some(Filter::IsNotNull {
        field: vec!["never".to_string()],
    });
    // `b_op`'s own `when` is TRUE (independent of `a`).
    req.queries.get_mut(b_op.alias()).unwrap().when = Some(Filter::IsNull {
        field: vec!["anything".to_string()],
    });

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    assert!(resp.results["a"].skipped, "a's own when must skip it");
    assert!(
        !resp.results["b_op"].skipped,
        "b_op must run per its OWN when (true), independent of the \
         after-only edge onto skipped 'a' — after-from-skipped is not a \
         cascade trigger"
    );
    assert!(
        resp.results["b_op"].stats.is_some(),
        "b_op must have actually executed"
    );

    // Now force b_op's own when to FALSE — it must skip on its own merit,
    // still independent of a's skip status (already skipped).
    let mut req2 = seed_req_for_after_own_when_false(&resolver).await;
    let _ = &mut req2;
}

/// Helper: rebuilds the same after+own-when scenario as the test above but
/// with `b_op`'s own `when` forced FALSE, to show `b_op` skips purely on
/// its own guard (not merely "because a was skipped").
async fn seed_req_for_after_own_when_false(
    resolver: &super::common::TestResolver,
) -> shamir_query_types::batch::BatchRequest {
    let mut b = Batch::new();
    b.id(3);
    let a = b.query("a", Query::from("users").where_eq("status", "active"));
    let b_op = b.op_silent(
        "b_op",
        write::insert("users").row(doc().set("name", "B2").set("status", "b-marker2")),
    );
    b.after(&b_op, &a);
    let mut req = b.build();
    req.queries.get_mut(a.alias()).unwrap().when = Some(Filter::IsNotNull {
        field: vec!["never".to_string()],
    });
    req.queries.get_mut(b_op.alias()).unwrap().when = Some(Filter::IsNotNull {
        field: vec!["never_either".to_string()],
    });

    let resp = execute_batch(&req, resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert!(resp.results["a"].skipped);
    assert!(
        resp.results["b_op"].skipped,
        "b_op must skip because its OWN when evaluated false, not because \
         'a' was skipped (after-only edges never cascade)"
    );

    req
}

/// Gap #4 (brief item 4): a 3-level DataFlow cascade — `C` depends on `B`
/// via `$query`, `B` depends on `A` via `$query`, `A` is skipped. Both `B`
/// AND `C` must cascade-skip (not just the immediate dependent `B`).
#[tokio::test]
async fn cascade_skip_propagates_through_three_level_dataflow_chain() {
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
    let bb = b.query(
        "b",
        Query::from("users").where_eq("name", a.first().field("name")),
    );
    let c = b.query(
        "c",
        Query::from("users").where_eq("name", bb.first().field("name")),
    );
    let mut req = b.build();

    // Force `a` to be skipped.
    req.queries.get_mut(a.alias()).unwrap().when = Some(Filter::IsNotNull {
        field: vec!["never".to_string()],
    });

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    assert!(resp.results["a"].skipped, "a's own when must skip it");
    assert!(
        resp.results["b"].skipped,
        "b (direct DataFlow dependent of skipped a) must cascade-skip"
    );
    assert!(
        resp.results["c"].skipped,
        "c (DataFlow dependent of skipped b, transitively of skipped a) \
         must ALSO cascade-skip through the full 3-level chain — not just \
         the immediate dependent"
    );
    let _ = &c;
}

/// Gap #5 (brief item 5): `skipped: true` (via `when: false`) is a
/// distinct concept from `return_only`/`return_flagged` filtering. A
/// skipped alias with `return_result: true` is PRESENT in `results` with
/// `skipped: true`; an alias with `return_result: false` (silent) is
/// ABSENT from `results` regardless of its `when`/`skipped` status.
#[tokio::test]
async fn skipped_status_is_distinct_from_return_result_filtering() {
    let resolver = setup_resolver().await;

    let mut b = Batch::new();
    b.id(1);
    b.return_flagged();
    // Non-silent (return_result: true) + when: false → present, skipped: true.
    b.query("visible_skipped", Query::from("users"));
    // Silent (return_result: false) + when: false → absent regardless.
    b.query_silent("hidden_skipped", Query::from("users"));
    // Silent (return_result: false) + when: true (executes) → still absent.
    b.query_silent("hidden_executed", Query::from("users"));
    let mut req = b.build();

    req.queries.get_mut("visible_skipped").unwrap().when = Some(Filter::IsNotNull {
        field: vec!["never".to_string()],
    });
    req.queries.get_mut("hidden_skipped").unwrap().when = Some(Filter::IsNotNull {
        field: vec!["never".to_string()],
    });
    // hidden_executed has no `when` — always executes.

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    assert!(
        resp.results.contains_key("visible_skipped"),
        "return_result: true alias must be present in results even when skipped"
    );
    assert!(
        resp.results["visible_skipped"].skipped,
        "visible_skipped must carry skipped: true"
    );
    assert!(
        !resp.results.contains_key("hidden_skipped"),
        "return_result: false alias must be ABSENT from results regardless \
         of its when/skipped status"
    );
    assert!(
        !resp.results.contains_key("hidden_executed"),
        "return_result: false alias must be ABSENT from results even when \
         it actually executed (return_only filtering is orthogonal to when)"
    );
}

/// Gap #7 (brief item 7): what actually happens when a `when` filter
/// references a `$query` alias that is never declared anywhere in the
/// batch.
///
/// Unlike Epic02's `$cond`-condition-missing-alias silent-miss (see
/// `cond_gap_tests.rs`'s `test_expr_arg_unresolvable_query_ref_is_none`,
/// which covers a `$query` ref that IS declared but whose path produced
/// no usable data), a `$query` ref to a genuinely UNDECLARED alias inside
/// `when` is caught by `BatchPlanner::plan`'s strict-validation pass
/// (`crates/shamir-query-types/src/batch/planner.rs` — "we use **strict
/// validation**... fails fast with a clear error instead of producing
/// wrong results"). The planner walks `when` for `$query` refs the same
/// way it walks `where`/`$cond`/`$expr`, and rejects any pointing at an
/// alias absent from `queries` — at PLAN time, before any op executes.
/// `execute_batch` therefore returns `Err(BatchError::UnknownAlias)` for
/// the WHOLE batch (no op runs, no `results` map is produced) — it does
/// NOT silently skip just the offending alias the way a bad `$cond`
/// condition would.
#[tokio::test]
async fn when_referencing_a_wholly_undeclared_alias_is_rejected_at_plan_time() {
    let resolver = setup_resolver().await;

    let mut b = Batch::new();
    b.id(1);
    b.query("probe", Query::from("users"));
    let mut req = b.build();

    // `$query` ref to an alias that is never declared anywhere in this
    // request. Uses `ValueCompare` (not a field-based variant like `Eq`) so
    // this test exercises the UnknownAlias plan-time check specifically,
    // not the separate #651 `InvalidWhenFilter` field-based-comparison
    // rejection (see `when_field_based_comparison_is_rejected_at_plan_time`
    // below for that check).
    req.queries.get_mut("probe").unwrap().when = Some(Filter::ValueCompare {
        left: shamir_query_types::filter::FilterValue::Int(1),
        cmp: shamir_query_types::filter::ValueCompareOp::Eq,
        right: shamir_query_types::filter::FilterValue::QueryRef {
            alias: "totally_undeclared".to_string(),
            path: Some("[0].y".to_string()),
        },
    });

    let err = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .expect_err(
            "a when filter referencing a wholly undeclared $query alias \
             must be rejected at plan time (strict validation), not \
             silently skip or panic",
        );

    match err {
        crate::query::batch::BatchError::UnknownAlias {
            alias,
            referenced_by,
        } => {
            assert_eq!(alias, "totally_undeclared");
            assert_eq!(referenced_by, "probe");
        }
        other => panic!("expected BatchError::UnknownAlias, got: {other:?}"),
    }
}

/// #651 fix: `Filter::ValueCompare` makes the ADR's own canonical scenario
/// — "run this op iff `$query_ref_A >= $query_ref_B`" (e.g. "debit iff
/// balance >= amount") — actually work, over BOTH directions. Unlike the
/// old field-based variants (which always folded to a fixed result against
/// the empty synthetic record — see `when_field_based_comparison_is_rejected...`
/// below), `ValueCompare` has no field/record dependency: both sides are
/// resolved via `resolve_filter_query` against `ctx.resolved_refs` at
/// match time, so a real cross-query comparison is finally reachable.
#[tokio::test]
async fn value_compare_makes_balance_gte_amount_scenario_work_sufficient_direction() {
    let resolver = setup_resolver().await;

    let mut seed = Batch::new();
    seed.id(1);
    seed.op_silent(
        "seed",
        write::insert("users").row(doc().set("name", "alice").set("balance", 100_i64)),
    );
    let seed_req = seed.build();
    execute_batch(&seed_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(2);
    let balance_check = b.query(
        "balance_check",
        Query::from("users").where_eq("name", "alice"),
    );
    let debit = b.op_silent(
        "debit",
        write::insert("users").row(doc().set("name", "debit-row")),
    );
    let mut req = b.build();

    // balance (100, via $query ref to balance_check's first row) >= amount
    // (literal 40) — should evaluate true, so `debit` must run.
    req.queries.get_mut(debit.alias()).unwrap().when = Some(Filter::ValueCompare {
        left: balance_check.first().field("balance"),
        cmp: ValueCompareOp::Gte,
        right: FilterValue::Int(40),
    });

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    assert!(
        !resp.results["debit"].skipped,
        "balance (100) >= amount (40) must evaluate true via ValueCompare \
         and execute the debit: {:?}",
        resp.results["debit"]
    );
}

/// Complementary direction of the scenario above: insufficient balance ->
/// `ValueCompare` evaluates false -> the op is skipped. Together the two
/// tests prove the comparison is genuinely data-driven, not "always true by
/// coincidence".
#[tokio::test]
async fn value_compare_makes_balance_gte_amount_scenario_work_insufficient_direction() {
    let resolver = setup_resolver().await;

    let mut seed = Batch::new();
    seed.id(1);
    seed.op_silent(
        "seed",
        write::insert("users").row(doc().set("name", "bob").set("balance", 10_i64)),
    );
    let seed_req = seed.build();
    execute_batch(&seed_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(2);
    let balance_check = b.query(
        "balance_check",
        Query::from("users").where_eq("name", "bob"),
    );
    let debit = b.op_silent(
        "debit",
        write::insert("users").row(doc().set("name", "debit-row")),
    );
    let mut req = b.build();

    // balance (10) >= amount (40) is false -> debit must be skipped.
    req.queries.get_mut(debit.alias()).unwrap().when = Some(Filter::ValueCompare {
        left: balance_check.first().field("balance"),
        cmp: ValueCompareOp::Gte,
        right: FilterValue::Int(40),
    });

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    assert!(
        resp.results["debit"].skipped,
        "balance (10) >= amount (40) must evaluate false via ValueCompare \
         and skip the debit: {:?}",
        resp.results["debit"]
    );
}

/// #651 safety net: an OLD field-based comparison variant (`Gte` here) used
/// inside `when` must be REJECTED at plan time with a clear
/// `BatchError::InvalidWhenFilter`, instead of silently folding to a fixed
/// result the way it did before this fix (`Gte`/`Eq`/etc. always folded to
/// `FilterNode::False` against the empty synthetic record's scratch
/// interner). This turns the old silent-wrong-answer bug into a caught,
/// explicit error that names the fix (`Filter::ValueCompare`).
#[tokio::test]
async fn when_field_based_comparison_is_rejected_at_plan_time() {
    let resolver = setup_resolver().await;

    let mut b = Batch::new();
    b.id(1);
    b.query("probe", Query::from("users"));
    let mut req = b.build();

    req.queries.get_mut("probe").unwrap().when = Some(Filter::Gte {
        field: vec!["balance".to_string()],
        value: FilterValue::Int(40),
    });

    let err = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .expect_err(
            "an old field-based comparison variant inside `when` must be \
             rejected at plan time, not silently folded to a fixed result",
        );

    match err {
        crate::query::batch::BatchError::InvalidWhenFilter { alias, message } => {
            assert_eq!(alias, "probe");
            assert!(
                message.contains("ValueCompare"),
                "error message must name the fix (Filter::ValueCompare): {message}"
            );
        }
        other => panic!("expected BatchError::InvalidWhenFilter, got: {other:?}"),
    }
}

/// `IsNull`/`IsNotNull` remain a legitimate presence-guard pattern inside
/// `when` (ADR Decision 1) — they must NOT be rejected by the new #651
/// defensive check, unlike the OLD field-based comparison variants above.
#[tokio::test]
async fn is_null_and_is_not_null_remain_accepted_inside_when() {
    let resolver = setup_resolver().await;

    let mut b = Batch::new();
    b.id(1);
    let probe = b.query("probe", Query::from("users"));
    let mut req = b.build();

    req.queries.get_mut(probe.alias()).unwrap().when = Some(Filter::IsNull {
        field: vec!["anything".to_string()],
    });

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .expect("IsNull inside `when` must remain accepted, not rejected");
    assert!(!resp.results["probe"].skipped);
}

/// Fix 2 (Finding 8) — a user-registered scalar referenced in a `when`
/// guard's `ValueCompare` must resolve correctly (not silently fall back to
/// builtins-only → Null → wrong skip/execute decision).
///
/// The `when` guard uses `$fn: my_double(21)` compared `Eq` against the
/// literal `42`. With the user scalar resolver threaded into `resolve_skip`
/// (via `.with_scalars(scalars)` at ~line 161 in `query_runner.rs`),
/// `my_double(21)` resolves to `Int(42)`, the `ValueCompare` evaluates
/// `42 == 42` → `true`, and the op executes normally.
///
/// **Pre-fix behavior** (builtins-only `ScalarResolver`): `my_double` is an
/// unknown function → `resolve_filter_query` returns `None` → the
/// `ValueCompare` arm `(None, _)` evaluates `false` for `Eq` → the op is
/// **skipped**. The test would fail at `!resp.results["probe"].skipped`
/// because the probe would be skipped instead of executed.
#[tokio::test]
async fn when_user_scalar_resolves_correctly_not_skipped() {
    let resolver = setup_resolver_with_scalars().await;

    let mut b = Batch::new();
    b.id(1);
    let probe = b.query("probe", Query::from("users"));
    let mut req = b.build();

    // `$fn: my_double(21)` == 42 → true (op executes).
    // my_double is a user-registered scalar that doubles its Int argument.
    req.queries.get_mut(probe.alias()).unwrap().when = Some(Filter::ValueCompare {
        left: FilterValue::FnCall {
            call: FnCall::complex("my_double", vec![FilterValue::Int(21)]),
        },
        cmp: ValueCompareOp::Eq,
        right: FilterValue::Int(42),
    });

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    assert!(
        !resp.results["probe"].skipped,
        "when: $fn my_double(21) == 42 must evaluate true (user scalar resolved) \
         and execute the op — if skipped, the user scalar was not threaded into \
         resolve_skip's FilterContext: {:?}",
        resp.results["probe"]
    );
}
