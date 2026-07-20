//! Test coverage for `FieldPathCache` (F1) — the pre-interned `FieldRef`
//! path cache.
//!
//! F1 mirrors `CondCache` (#643): `resolve_filter_query`'s `FieldRef` arm
//! used to call `intern_field_path(path, ctx.interner)` on EVERY record
//! (one `Vec<u64>` alloc + one `Interner::get_ind` `DashMap` lookup per
//! path segment, per row), even though `path` is static per query. This
//! cache keys the pre-interned `SmallVec<[InternerKey; 4]>` by pointer
//! identity of the `FieldRef` node itself, so callers that pre-scan a
//! static `FilterValue` tree once (`SelectProjection::new`) skip the
//! per-row re-interning entirely.
//!
//! Part A (tests 1-3): direct unit tests for `prescan_field_path_cache`'s
//! structural / pointer-identity behaviour.
//! Part B (tests 4-5): the DECISIVE tests proving the cache-HIT path and
//! the cache-MISS path (`field_path_cache: None`) produce IDENTICAL
//! results for the same `SELECT $fn($ref)`-shaped query — the brief's
//! mandatory regression proof.
//! Part C (test 6): the engine-level integration test through the real
//! production seam (`SelectProjection::new` → `project_value`), proving
//! the cache is wired into the one call site this brief touches.

use std::sync::Arc;

use shamir_funclib::scalar_resolver::ScalarResolver;
use shamir_types::core::interner::Interner;
use shamir_types::types::common::{new_map, TMap};
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::filter::eval::resolve_filter_query;
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::field_path_cache::{prescan_field_path_cache, FieldPathCache};
use crate::query::filter::{Cond, Filter, FilterValue, FnCall};
use crate::query::read::select_projection::SelectProjection;
use crate::query::read::{QueryResult, Select, SelectItem};

fn empty_refs() -> TMap<String, QueryResult> {
    new_map()
}

/// Build a record `{name: <name>}` with `name` interned.
fn make_name_record(interner: &Interner, name: &str) -> InnerValue {
    let mut map = new_map();
    let k_name = interner.touch_ind("name").unwrap().into_key();
    map.insert(k_name, InnerValue::Str(name.to_string()));
    InnerValue::Map(map)
}

/// Build a record `{name: <name>, score: <score>}` with both fields interned.
fn make_record_2(interner: &Interner, name: &str, score: i64) -> InnerValue {
    let mut map = new_map();
    let k_name = interner.touch_ind("name").unwrap().into_key();
    let k_score = interner.touch_ind("score").unwrap().into_key();
    map.insert(k_name, InnerValue::Str(name.to_string()));
    map.insert(k_score, InnerValue::Int(score));
    InnerValue::Map(map)
}

// ---------------------------------------------------------------------
// Part A — direct unit tests for prescan_field_path_cache
// ---------------------------------------------------------------------

/// Test 1: `prescan_field_path_cache` populates the cache for the simplest
/// possible `FieldRef` — a single-segment path.
#[test]
fn prescan_populates_simple_field_ref() {
    let interner = Interner::new();
    interner.touch_ind("name").unwrap();

    let fv = FilterValue::field_ref("name");

    let mut cache: FieldPathCache = new_map();
    prescan_field_path_cache(&fv, &interner, &mut cache);

    // Destructure the SAME owned tree the cache was built from — pointer
    // identity is the cache key, so looking up via a cloned/rebuilt `FieldRef`
    // would (correctly) miss. See `FieldPathCache`'s doc comment.
    assert!(
        cache.get(&(fv_ref_addr(&fv))).is_some(),
        "prescan_field_path_cache must populate the cache for the FieldRef's node"
    );
}

/// Test 2: pointer-identity miss path: two INDEPENDENT `FieldRef`s
/// (distinct allocations, distinct addresses). Only the first is
/// prescanned; looking up the second's node must miss.
#[test]
fn cache_misses_unregistered_field_ref_node() {
    let interner = Interner::new();
    interner.touch_ind("name").unwrap();

    let first = FilterValue::field_ref("name");
    let second = FilterValue::field_ref("name"); // structurally identical, DIFFERENT allocation

    let mut cache: FieldPathCache = new_map();
    prescan_field_path_cache(&first, &interner, &mut cache);

    // `second` was never prescanned — its node lives at a different address
    // than `first`'s, so pointer-identity lookup must miss.
    assert!(
        cache.get(&(fv_ref_addr(&second))).is_none(),
        "cache must miss for a FieldRef node that was never prescanned \
         (distinct pointer, even if structurally identical)"
    );
}

/// Test 3: `prescan_field_path_cache` recurses into every documented shape
/// that can embed a nested `FieldRef` (FnCall args, Expr args, Cond branches,
/// Cond condition's Filter operands). Table-driven: each case nests a
/// `FieldRef` at a distinct position and asserts the nested node IS found
/// after a single prescan of the outer tree.
#[test]
fn prescan_recurses_into_all_documented_shapes() {
    let interner = Interner::new();
    interner.touch_ind("name").unwrap();

    // Case 1: inside FilterValue::FnCall's args (the SELECT upper(name) shape).
    {
        let nested_fv = FilterValue::field_ref("name");
        let fv = FilterValue::FnCall {
            call: FnCall::complex("strings/upper", vec![nested_fv]),
        };
        let mut cache: FieldPathCache = new_map();
        prescan_field_path_cache(&fv, &interner, &mut cache);
        let FilterValue::FnCall { call } = &fv else {
            unreachable!()
        };
        assert!(
            cache.get(&(fv_ref_addr(&call.args()[0]))).is_some(),
            "FnCall arg FieldRef must be prescanned"
        );
    }

    // Case 2: inside FilterValue::Expr's args.
    {
        let nested_fv = FilterValue::field_ref("name");
        let fv = FilterValue::Expr {
            expr: crate::query::filter::FilterExpr::concat(vec![
                nested_fv,
                FilterValue::String("!".to_string()),
            ]),
        };
        let mut cache: FieldPathCache = new_map();
        prescan_field_path_cache(&fv, &interner, &mut cache);
        let FilterValue::Expr { expr } = &fv else {
            unreachable!()
        };
        assert!(
            cache.get(&(fv_ref_addr(&expr.args[0]))).is_some(),
            "Expr arg FieldRef must be prescanned"
        );
    }

    // Case 3: inside a Cond's `then` branch (a $cond whose then is a FieldRef).
    {
        let inner_fv = FilterValue::field_ref("name");
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
        let mut cache: FieldPathCache = new_map();
        prescan_field_path_cache(&fv, &interner, &mut cache);
        let FilterValue::Cond { cond } = &fv else {
            unreachable!()
        };
        assert!(
            cache.get(&(fv_ref_addr(&cond.then))).is_some(),
            "Cond's own `then` branch nested FieldRef must be prescanned"
        );
    }

    // Case 4: inside a Cond's `or_else` branch.
    {
        let inner_fv = FilterValue::field_ref("name");
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
        let mut cache: FieldPathCache = new_map();
        prescan_field_path_cache(&fv, &interner, &mut cache);
        let FilterValue::Cond { cond } = &fv else {
            unreachable!()
        };
        assert!(
            cache.get(&(fv_ref_addr(&cond.or_else))).is_some(),
            "Cond's own `or_else` branch nested FieldRef must be prescanned"
        );
    }

    // Case 5: inside the condition Filter itself, at an embedded FilterValue
    // operand — Filter::Eq { value: <nested FieldRef> }. Exercises
    // prescan_filter's walk.
    {
        let inner_fv = FilterValue::field_ref("name");
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
        let mut cache: FieldPathCache = new_map();
        prescan_field_path_cache(&fv, &interner, &mut cache);
        let FilterValue::Cond { cond } = &fv else {
            unreachable!()
        };
        let Filter::Eq { value, .. } = cond.condition.as_ref() else {
            unreachable!()
        };
        assert!(
            cache.get(&(fv_ref_addr(value))).is_some(),
            "FieldRef embedded inside Filter::Eq's `value` operand must be prescanned \
             via prescan_filter"
        );
    }
}

/// Extract the pointer-identity key for a `FilterValue` reference, matching
/// the key `prescan_field_path_cache` inserts (`fv as *const FilterValue as
/// usize`). Avoids raw pointers/`unsafe` in the test body.
fn fv_ref_addr(fv: &FilterValue) -> usize {
    fv as *const FilterValue as usize
}

// ---------------------------------------------------------------------
// Part B — DECISIVE tests: cache-HIT and cache-MISS paths produce
// IDENTICAL results (the brief's mandatory regression proof).
// ---------------------------------------------------------------------

/// Test 4: THE decisive equivalence test (brief Verification §2). A
/// `FilterValue::FnCall { strings/upper, [$ref name] }` projection node is
/// evaluated TWICE for the same record:
///   (a) through a `FilterContext` carrying a populated `FieldPathCache`
///       (the cache-HIT path — `SelectProjection`'s shape),
///   (b) through a bare `FilterContext::new(...)` with `field_path_cache:
///       None` (the cache-MISS fallback path — every EXISTING caller's
///       shape, unchanged by F1).
/// Both must return IDENTICAL `QueryValue`s. If the cache ever stored a
/// wrong path (e.g. corrupted by a stale interner state) or the hit/miss
/// branches diverged in semantics, this assertion would fail.
#[test]
fn cached_and_uncached_field_ref_produce_identical_results() {
    let interner = Interner::new();
    interner.touch_ind("name").unwrap();

    // Build the SAME FnCall projection node ONCE. It must be reused for both
    // paths (pointer identity is the cache key) — a fresh clone for path (b)
    // would defeat the test (both would be misses).
    let fv = FilterValue::FnCall {
        call: FnCall::complex("strings/upper", vec![FilterValue::field_ref("name")]),
    };

    // --- Path (a): cache populated via prescan_field_path_cache ---
    let mut cache: FieldPathCache = new_map();
    prescan_field_path_cache(&fv, &interner, &mut cache);
    // Sanity: the nested FieldRef was actually cached (else path (a) silently
    // degenerates to a miss and the test proves nothing).
    let nested_addr = {
        let FilterValue::FnCall { call } = &fv else {
            unreachable!()
        };
        fv_ref_addr(&call.args()[0])
    };
    assert!(
        cache.get(&nested_addr).is_some(),
        "sanity: prescan must have cached the nested FieldRef for path (a)"
    );

    let refs = empty_refs();
    let ctx_cached = FilterContext::new(&interner, &refs).with_field_path_cache(&cache);
    let record = make_name_record(&interner, "alice");

    let result_cached = resolve_filter_query(&fv, &record, &ctx_cached);

    // --- Path (b): bare FilterContext (field_path_cache: None) ---
    let ctx_uncached = FilterContext::new(&interner, &refs);
    let result_uncached = resolve_filter_query(&fv, &record, &ctx_uncached);

    assert_eq!(
        result_cached, result_uncached,
        "cache-HIT and cache-MISS paths must produce IDENTICAL QueryValues for \
         the same SELECT $fn($ref) projection. cached={:?} uncached={:?}",
        result_cached, result_uncached
    );
    // And the shared value is the expected upper-cased name.
    assert_eq!(
        result_cached,
        Some(QueryValue::Str("ALICE".to_string())),
        "both paths must resolve upper(name) => \"ALICE\""
    );
}

/// Test 5: the equivalence from test 4 holds across MULTIPLE records with
/// DIFFERING field values — guards against a regression where the cached
/// path freezes the FIRST record's resolved value (rather than re-resolving
/// `materialize_at` per record via the cached interned path).
#[test]
fn cached_path_evaluates_per_record_not_stale() {
    let interner = Interner::new();
    interner.touch_ind("name").unwrap();

    let fv = FilterValue::FnCall {
        call: FnCall::complex("strings/upper", vec![FilterValue::field_ref("name")]),
    };

    let mut cache: FieldPathCache = new_map();
    prescan_field_path_cache(&fv, &interner, &mut cache);

    let refs = empty_refs();
    let ctx_cached = FilterContext::new(&interner, &refs).with_field_path_cache(&cache);
    let ctx_uncached = FilterContext::new(&interner, &refs);

    for name in ["alice", "bob", "carol"] {
        let record = make_name_record(&interner, name);
        let cached = resolve_filter_query(&fv, &record, &ctx_cached);
        let uncached = resolve_filter_query(&fv, &record, &ctx_uncached);
        assert_eq!(
            cached, uncached,
            "per-record divergence for name={name}: cached={cached:?} uncached={uncached:?}"
        );
        assert_eq!(
            cached,
            Some(QueryValue::Str(name.to_uppercase())),
            "both paths must resolve upper({name}) correctly"
        );
    }
}

// ---------------------------------------------------------------------
// Part C — engine-level integration through the production seam.
// ---------------------------------------------------------------------

/// Test 6: the real production wiring path — `SelectProjection::new` (which
/// calls `prescan_field_path_cache` internally per F1 step 5) builds the
/// cache ONCE, then `project_value` serves TWO records with differing field
/// values through the SAME projection. Mirrors `cond_cache_tests.rs`'s test 5
/// / `select_projection_tests.rs`'s `$cond` projection test.
///
/// If the cache wiring froze the first record's `name` (rather than
/// re-resolving via the cached interned path per call), the second record's
/// assertion would still see "ALICE" instead of "BOB".
#[test]
fn select_projection_caches_field_ref_and_evaluates_per_record() {
    let interner = Arc::new(Interner::new());
    interner.touch_ind("name").unwrap();

    // SELECT upper(name) AS upper_name
    let select = Select {
        items: vec![SelectItem::Function {
            name: "strings/upper".to_string(),
            args: vec![FilterValue::field_ref("name")],
            alias: Some("upper_name".to_string()),
        }],
        distinct: false,
    };

    // Built ONCE — this populates `funcs_field_path_cache` internally via
    // `prescan_field_path_cache`, the real production call site under test.
    let proj = SelectProjection::new(&select, &interner, ScalarResolver::builtins_only());

    let record_a = make_record_2(&interner, "alice", 0);
    let record_b = make_record_2(&interner, "bob", 0);

    let qval_a = proj.project_value(&record_a, &interner);
    let qval_b = proj.project_value(&record_b, &interner);

    match &qval_a {
        QueryValue::Map(m) => {
            assert_eq!(
                m.get("upper_name"),
                Some(&QueryValue::Str("ALICE".to_string())),
                "first record (name=alice) must project to \"ALICE\" via the cached FieldRef path"
            );
        }
        _ => panic!("expected QueryValue::Map, got {:?}", qval_a),
    }
    match &qval_b {
        QueryValue::Map(m) => {
            assert_eq!(
                m.get("upper_name"),
                Some(&QueryValue::Str("BOB".to_string())),
                "second record (name=bob) must project to \"BOB\" via the SAME cached \
                 FieldRef path — proving the cache is genuinely re-resolved per record, \
                 not frozen to the first record's answer"
            );
        }
        _ => panic!("expected QueryValue::Map, got {:?}", qval_b),
    }
}
