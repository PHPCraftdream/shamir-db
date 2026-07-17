//! Test coverage for `CondCache`'s cache-hit path (#665 — #643 gap).
//!
//! #643 added a pointer-keyed cache (`CondCache`) mapping a `$cond`'s
//! `condition: Box<Filter>` (by raw address) to its pre-compiled
//! `FilterNode`, so `resolve_filter_query`'s `FilterValue::Cond` arm can
//! reuse a compiled node across many records instead of recompiling
//! `cond.condition` on every row. Before this file, ZERO test exercised any
//! of `prescan_cond_cache`, `cond_cache_get`, or — most importantly — the
//! actual cache-HIT branch inside `resolve_filter_query` (`Some(node) =>
//! node.matches(record, ctx)`). This is exactly the class of gap a
//! performance optimization can hide: the cache could silently freeze a
//! `$cond`'s answer to whichever record happened to populate it, and
//! nothing would catch it, because nothing exercised the hit path at all.
//!
//! Part A (tests 1-4): direct unit tests for `prescan_cond_cache` /
//! `cond_cache_get`'s structural/pointer-identity behaviour.
//! Part B (tests 5-7): the decisive tests proving the cache-HIT path
//! re-evaluates `node.matches(record, ctx)` per call rather than baking in
//! a frozen answer at cache-population time.

use shamir_types::core::interner::Interner;
use shamir_types::types::common::new_map;
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::filter::cond_cache::{cond_cache_get, prescan_cond_cache, CondCache};
use crate::query::filter::eval::resolve_filter_query;
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::{Cond, Filter, FilterExpr, FilterValue, FnCall};
use crate::query::read::QueryResult;
use shamir_types::types::common::TMap;

fn empty_refs() -> TMap<String, QueryResult> {
    new_map()
}

/// Build a record `{score: <score>}` with `score` interned.
fn make_score_record(interner: &Interner, score: i64) -> InnerValue {
    let mut map = new_map();
    let k_score = interner.touch_ind("score").unwrap().into_key();
    map.insert(k_score, InnerValue::Int(score));
    InnerValue::Map(map)
}

/// A `$cond` testing `score > 50 ? "high" : "low"`.
fn make_score_cond() -> Cond {
    Cond::new(
        Filter::Gt {
            field: vec!["score".to_string()],
            value: FilterValue::Int(50),
        },
        FilterValue::String("high".to_string()),
        FilterValue::String("low".to_string()),
    )
}

// ---------------------------------------------------------------------
// Part A — direct unit tests for prescan_cond_cache / cond_cache_get
// ---------------------------------------------------------------------

/// Test 1: `prescan_cond_cache` populates the cache for the simplest possible
/// `$cond` — a trivial `IsNotNull` condition, no record evaluation needed.
#[test]
fn prescan_populates_simple_cond() {
    let interner = Interner::new();
    let cond = Cond::new(
        Filter::IsNotNull {
            field: vec!["x".to_string()],
        },
        FilterValue::Bool(true),
        FilterValue::Bool(false),
    );
    let fv = FilterValue::Cond {
        cond: Box::new(cond),
    };

    let mut cache: CondCache = new_map();
    prescan_cond_cache(&fv, &interner, &mut cache);

    // Destructure the SAME owned tree the cache was built from — pointer
    // identity is the cache key, so looking up via a cloned/rebuilt `Cond`
    // would (correctly) miss. See `CondCache`'s doc comment.
    let inner_cond = as_cond(&fv);
    assert!(
        cond_cache_get(&cache, &inner_cond.condition).is_some(),
        "prescan_cond_cache must populate the cache for the Cond's condition"
    );
}

/// Test 2: `cond_cache_get`'s pointer-identity miss path: two INDEPENDENT `Cond`s
/// (distinct `Box<Filter>` allocations, distinct addresses). Only the first
/// is prescanned; looking up the second's condition must miss.
#[test]
fn cond_cache_get_misses_unregistered_condition() {
    let interner = Interner::new();
    let first = make_score_cond();
    let second = make_score_cond(); // structurally identical, but a DIFFERENT allocation

    let fv_first = FilterValue::Cond {
        cond: Box::new(first),
    };

    let mut cache: CondCache = new_map();
    prescan_cond_cache(&fv_first, &interner, &mut cache);

    // `second` was never prescanned — its condition lives at a different
    // address than `fv_first`'s, so pointer-identity lookup must miss.
    assert!(
        cond_cache_get(&cache, &second.condition).is_none(),
        "cond_cache_get must miss for a condition that was never prescanned \
         (distinct pointer, even if structurally identical)"
    );
}

/// Extract the innermost nested `Cond`'s `condition` reference back out of a
/// `FilterValue::Cond` wrapper, for pointer-identity lookup after prescan.
/// Avoids raw pointers/`unsafe` — just destructures the same owned tree the
/// cache was built from.
fn as_cond(fv: &FilterValue) -> &Cond {
    match fv {
        FilterValue::Cond { cond } => cond,
        _ => panic!("expected FilterValue::Cond"),
    }
}

/// Test 3: `prescan_cond_cache` recurses into every documented shape that can
/// embed a nested `$cond`. Table-driven: each case nests a `$cond` at a
/// distinct position and asserts the nested Cond's condition IS found via
/// `cond_cache_get` after a single prescan of the outer tree.
#[test]
fn prescan_recurses_into_all_documented_shapes() {
    let interner = Interner::new();

    // Case 1: inside FilterValue::Array (an array element is a Cond).
    {
        let nested_fv = FilterValue::Cond {
            cond: Box::new(make_score_cond()),
        };
        let fv = FilterValue::Array(vec![FilterValue::Int(1), nested_fv]);
        let mut cache: CondCache = new_map();
        prescan_cond_cache(&fv, &interner, &mut cache);
        let FilterValue::Array(items) = &fv else {
            unreachable!()
        };
        assert!(
            cond_cache_get(&cache, &as_cond(&items[1]).condition).is_some(),
            "Array element Cond must be prescanned"
        );
    }

    // Case 2: inside FilterValue::FnCall's args.
    {
        let nested_fv = FilterValue::Cond {
            cond: Box::new(make_score_cond()),
        };
        let fv = FilterValue::FnCall {
            call: FnCall::complex("strings/upper", vec![nested_fv]),
        };
        let mut cache: CondCache = new_map();
        prescan_cond_cache(&fv, &interner, &mut cache);
        let FilterValue::FnCall { call } = &fv else {
            unreachable!()
        };
        assert!(
            cond_cache_get(&cache, &as_cond(&call.args()[0]).condition).is_some(),
            "FnCall arg Cond must be prescanned"
        );
    }

    // Case 3: inside FilterValue::Expr's args.
    {
        let nested_fv = FilterValue::Cond {
            cond: Box::new(make_score_cond()),
        };
        let fv = FilterValue::Expr {
            expr: FilterExpr::add(vec![FilterValue::Int(1), nested_fv]),
        };
        let mut cache: CondCache = new_map();
        prescan_cond_cache(&fv, &interner, &mut cache);
        let FilterValue::Expr { expr } = &fv else {
            unreachable!()
        };
        assert!(
            cond_cache_get(&cache, &as_cond(&expr.args[1]).condition).is_some(),
            "Expr arg Cond must be prescanned"
        );
    }

    // Case 4: inside a Cond's OWN `then` branch (a $cond whose then is
    // itself another $cond).
    {
        let inner_fv = FilterValue::Cond {
            cond: Box::new(make_score_cond()),
        };
        let outer = Cond::new(
            Filter::IsNotNull {
                field: vec!["x".to_string()],
            },
            inner_fv,
            FilterValue::String("else".to_string()),
        );
        let fv = FilterValue::Cond {
            cond: Box::new(outer),
        };
        let mut cache: CondCache = new_map();
        prescan_cond_cache(&fv, &interner, &mut cache);
        let outer_cond = as_cond(&fv);
        assert!(
            cond_cache_get(&cache, &as_cond(&outer_cond.then).condition).is_some(),
            "Cond's own `then` branch nested Cond must be prescanned"
        );
    }

    // Case 5: inside a Cond's OWN `or_else` branch.
    {
        let inner_fv = FilterValue::Cond {
            cond: Box::new(make_score_cond()),
        };
        let outer = Cond::new(
            Filter::IsNotNull {
                field: vec!["x".to_string()],
            },
            FilterValue::String("then".to_string()),
            inner_fv,
        );
        let fv = FilterValue::Cond {
            cond: Box::new(outer),
        };
        let mut cache: CondCache = new_map();
        prescan_cond_cache(&fv, &interner, &mut cache);
        let outer_cond = as_cond(&fv);
        assert!(
            cond_cache_get(&cache, &as_cond(&outer_cond.or_else).condition).is_some(),
            "Cond's own `or_else` branch nested Cond must be prescanned"
        );
    }

    // Case 6: inside the condition Filter itself, at an embedded FilterValue
    // operand — Filter::Eq { value: <nested Cond as FilterValue> } as the
    // OUTER Cond's condition. Exercises prescan_filter's walk.
    {
        let inner_fv = FilterValue::Cond {
            cond: Box::new(make_score_cond()),
        };
        let outer = Cond::new(
            Filter::Eq {
                field: vec!["x".to_string()],
                value: inner_fv,
            },
            FilterValue::String("then".to_string()),
            FilterValue::String("else".to_string()),
        );
        let fv = FilterValue::Cond {
            cond: Box::new(outer),
        };
        let mut cache: CondCache = new_map();
        prescan_cond_cache(&fv, &interner, &mut cache);
        let outer_cond = as_cond(&fv);
        let Filter::Eq { value, .. } = outer_cond.condition.as_ref() else {
            unreachable!()
        };
        assert!(
            cond_cache_get(&cache, &as_cond(value).condition).is_some(),
            "Cond embedded inside Filter::Eq's `value` operand must be prescanned \
             via prescan_filter"
        );
    }

    // Case 7: another prescan_filter arm — Filter::In { values: [<nested
    // Cond>], .. } — proves the walk isn't special-cased to just Eq.
    {
        let inner_fv = FilterValue::Cond {
            cond: Box::new(make_score_cond()),
        };
        let outer = Cond::new(
            Filter::In {
                field: vec!["x".to_string()],
                values: vec![inner_fv],
            },
            FilterValue::String("then".to_string()),
            FilterValue::String("else".to_string()),
        );
        let fv = FilterValue::Cond {
            cond: Box::new(outer),
        };
        let mut cache: CondCache = new_map();
        prescan_cond_cache(&fv, &interner, &mut cache);
        let outer_cond = as_cond(&fv);
        let Filter::In { values, .. } = outer_cond.condition.as_ref() else {
            unreachable!()
        };
        assert!(
            cond_cache_get(&cache, &as_cond(&values[0]).condition).is_some(),
            "Cond embedded inside Filter::In's `values` operand must be prescanned \
             via prescan_filter"
        );
    }

    // Case 8: Filter::ValueCompare { left: <nested Cond>, .. } — yet another
    // prescan_filter arm.
    {
        let inner_fv = FilterValue::Cond {
            cond: Box::new(make_score_cond()),
        };
        let outer = Cond::new(
            Filter::ValueCompare {
                left: inner_fv,
                cmp: shamir_query_types::filter::ValueCompareOp::Eq,
                right: FilterValue::Int(1),
            },
            FilterValue::String("then".to_string()),
            FilterValue::String("else".to_string()),
        );
        let fv = FilterValue::Cond {
            cond: Box::new(outer),
        };
        let mut cache: CondCache = new_map();
        prescan_cond_cache(&fv, &interner, &mut cache);
        let outer_cond = as_cond(&fv);
        let Filter::ValueCompare { left, .. } = outer_cond.condition.as_ref() else {
            unreachable!()
        };
        assert!(
            cond_cache_get(&cache, &as_cond(left).condition).is_some(),
            "Cond embedded inside Filter::ValueCompare's `left` operand must be \
             prescanned via prescan_filter"
        );
    }
}

/// Test 4: Repeated `prescan_cond_cache` calls on the SAME `FilterValue` tree into
/// the SAME cache are idempotent: still exactly one entry for that
/// condition's pointer, and `cond_cache_get` still resolves it correctly.
/// Guards the `or_insert_with` idempotency the doc comment implies but never
/// tests.
#[test]
fn repeated_prescan_of_same_condition_is_idempotent() {
    let interner = Interner::new();
    let cond = make_score_cond();
    let fv = FilterValue::Cond {
        cond: Box::new(cond),
    };

    let mut cache: CondCache = new_map();
    prescan_cond_cache(&fv, &interner, &mut cache);
    let len_after_first = cache.len();
    assert_eq!(len_after_first, 1);

    // Prescan the SAME tree again into the SAME cache.
    prescan_cond_cache(&fv, &interner, &mut cache);
    assert_eq!(
        cache.len(),
        len_after_first,
        "repeated prescan of the same condition must not grow the cache"
    );

    // The FilterValue::Cond { cond } destructures back out to inspect
    // the address that was originally cached.
    let FilterValue::Cond { cond } = &fv else {
        unreachable!()
    };
    assert!(
        cond_cache_get(&cache, &cond.condition).is_some(),
        "cond_cache_get must still resolve the (idempotently re-cached) condition"
    );
}

// ---------------------------------------------------------------------
// Part B — the decisive tests: cache-HIT path produces correct,
// per-record results (not a frozen/stale answer).
// ---------------------------------------------------------------------

/// Test 5: THE decisive test: a SINGLE `$cond` (score > 50 ? "high" : "low"),
/// prescanned into a `CondCache` ONCE, evaluated via a `FilterContext` that
/// carries that cache (`with_cond_cache`) for TWO records with DIFFERING
/// `score` values straddling the threshold. If the cache-hit branch ever
/// regressed to freeze the FIRST record's answer (e.g. returning a stored
/// bool/QueryValue instead of calling `node.matches(record, ctx)` again),
/// the SECOND record's assertion below would fail — it would still see
/// "high" instead of "low". These two differing expected outputs are
/// load-bearing, not incidental.
#[test]
fn cached_cond_evaluates_correctly_per_record_not_stale() {
    let interner = Interner::new();
    // `compile_filter`'s field-path resolution (`intern_field_path_compact`
    // → `Interner::get_ind`) is lookup-only — it never inserts. "score" must
    // already be interned before `prescan_cond_cache` compiles the
    // condition, or the field path fails to resolve and the compiled node
    // folds to `FilterNode::False` (always "low"), independent of any
    // record. Mirrors how production interning happens (write path interns
    // field names) before a query ever prescans/compiles a filter over them.
    interner.touch_ind("score").unwrap();

    let cond = make_score_cond();
    let fv = FilterValue::Cond {
        cond: Box::new(cond),
    };

    let mut cache: CondCache = new_map();
    prescan_cond_cache(&fv, &interner, &mut cache);

    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs).with_cond_cache(&cache);

    let record_high = make_score_record(&interner, 80);
    let record_low = make_score_record(&interner, 20);

    // First call populates nothing new (cache already built) but exercises
    // the hit path for score=80 (> 50 => "high").
    assert_eq!(
        resolve_filter_query(&fv, &record_high, &ctx),
        Some(QueryValue::Str("high".to_string())),
        "first record (score=80) must resolve to \"high\" via the cached FilterNode"
    );

    // SAME FilterValue, SAME cache, SAME ctx — but a DIFFERENT record
    // (score=20 <= 50 => "low"). If the cached FilterNode's `matches` were
    // not re-invoked per call (a frozen/stale-answer regression), this
    // would incorrectly still return "high".
    assert_eq!(
        resolve_filter_query(&fv, &record_low, &ctx),
        Some(QueryValue::Str("low".to_string())),
        "second record (score=20) must resolve to \"low\" via the SAME cached \
         FilterNode — proving it is genuinely re-evaluated per record, not frozen \
         to the first record's answer"
    );
}

/// Test 6: The SAME condition/records as test 5, but the `FilterContext` is built
/// WITHOUT `.with_cond_cache(..)` (the default, `cond_cache: None`) — the
/// SAME correct high/low results must come out via the `compile_filter`
/// fallback branch. Regression pairing: cache-populated and cache-absent
/// paths must agree.
#[test]
fn uncached_cond_still_resolves_correctly_via_fallback() {
    let interner = Interner::new();
    let cond = make_score_cond();
    let fv = FilterValue::Cond {
        cond: Box::new(cond),
    };

    let refs = empty_refs();
    // No .with_cond_cache(..) — ctx.cond_cache stays None (default).
    let ctx = FilterContext::new(&interner, &refs);

    let record_high = make_score_record(&interner, 80);
    let record_low = make_score_record(&interner, 20);

    assert_eq!(
        resolve_filter_query(&fv, &record_high, &ctx),
        Some(QueryValue::Str("high".to_string())),
        "uncached path: score=80 must resolve to \"high\""
    );
    assert_eq!(
        resolve_filter_query(&fv, &record_low, &ctx),
        Some(QueryValue::Str("low".to_string())),
        "uncached path: score=20 must resolve to \"low\""
    );
}

/// Test 7: A `CondCache` populated with a DIFFERENT `Cond`'s condition (distinct
/// pointer) is threaded into the context, but the `Cond` actually being
/// evaluated was NEVER prescanned — `cond_cache_get` must miss for THIS
/// condition's pointer and fall back to `compile_filter`, still producing
/// correct results. Distinct from test 6 ("no cache at all"): here the
/// cache exists and is non-empty, just irrelevant to this particular Cond.
#[test]
fn cache_populated_for_other_conditions_still_falls_back_correctly() {
    let interner = Interner::new();

    // First, independent Cond — this is the ONLY one prescanned into the cache.
    let other_cond = Cond::new(
        Filter::IsNotNull {
            field: vec!["unrelated".to_string()],
        },
        FilterValue::Bool(true),
        FilterValue::Bool(false),
    );
    let other_fv = FilterValue::Cond {
        cond: Box::new(other_cond),
    };
    let mut cache: CondCache = new_map();
    prescan_cond_cache(&other_fv, &interner, &mut cache);
    assert_eq!(cache.len(), 1, "sanity: cache holds only the OTHER Cond");

    // Second, independent Cond (the score cond) — NEVER prescanned.
    let cond = make_score_cond();
    let fv = FilterValue::Cond {
        cond: Box::new(cond),
    };

    let refs = empty_refs();
    // Context carries the (irrelevantly-populated) cache.
    let ctx = FilterContext::new(&interner, &refs).with_cond_cache(&cache);

    let record_high = make_score_record(&interner, 80);
    let record_low = make_score_record(&interner, 20);

    assert_eq!(
        resolve_filter_query(&fv, &record_high, &ctx),
        Some(QueryValue::Str("high".to_string())),
        "cache-miss-for-this-Cond must still fall back to compile_filter and \
         resolve correctly (score=80 => \"high\")"
    );
    assert_eq!(
        resolve_filter_query(&fv, &record_low, &ctx),
        Some(QueryValue::Str("low".to_string())),
        "cache-miss-for-this-Cond must still fall back to compile_filter and \
         resolve correctly (score=20 => \"low\")"
    );
}
