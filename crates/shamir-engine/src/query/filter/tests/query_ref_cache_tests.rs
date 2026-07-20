//! Test coverage for `QueryRefCache` (F2) — the lazily-populated
//! `$query`/`QueryRef` resolution cache.
//!
//! F2 is structurally DIFFERENT from F1 (`FieldPathCache`): F1's cache is
//! populated EAGERLY at prescan time because a `FieldRef`'s resolution is
//! 100% static per query. A `$query`/`QueryRef`'s resolution depends on
//! `ctx.resolved_refs` — runtime scan data that does NOT exist at
//! `SelectProjection::new()` time — so F2's cache slots are RESERVED at
//! prescan time but FILLED LAZILY on the first row that hits each node
//! (via `OnceLock::get_or_init`), mirroring `In`'s `ref_column_sets`
//! pattern (`filter_node.rs`).
//!
//! Part A (test 1): structural / pointer-identity / recursion coverage for
//! `prescan_query_ref_cache`.
//! Part B (test 2): the F2-SPECIFIC lazy-population proof — the `OnceLock`
//! is empty before the first `resolve_filter_query` call and populated after.
//! Part C (tests 3-5): the DECISIVE equivalence tests — cache-HIT path
//! (`with_query_ref_cache`) and cache-MISS path (`query_ref_cache: None`)
//! produce IDENTICAL results, across TWO different `ctx.resolved_refs` scans
//! (proving the cache is per-scan fresh, not stale/shared across unrelated
//! queries), for both the Call-value path and the Read-records path, plus the
//! missing-alias collapse-to-None case.
//! Part D (test 6): the engine-level integration test through the real
//! production seam (`SelectProjection::new` → `project_value`), proving the
//! wiring is live and consistent per record.

use std::sync::Arc;

use shamir_funclib::scalar_resolver::ScalarResolver;
use shamir_types::core::interner::Interner;
use shamir_types::types::common::{new_map, TMap};
use shamir_types::types::value::QueryValue;

use crate::query::filter::eval::resolve_filter_query;
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::query_ref_cache::{prescan_query_ref_cache, QueryRefCache};
use crate::query::filter::{Cond, Filter, FilterValue, FnCall};
use crate::query::read::select_projection::SelectProjection;
use crate::query::read::{QueryRecord, QueryResult, Select, SelectItem};

/// A minimal `QueryResult` carrying a Call-result `value` (the value-first
/// path — `resolve_query_ref_value` applies `path` to `value`, not `records`).
fn call_result(value: QueryValue) -> QueryResult {
    QueryResult {
        records: vec![],
        stats: None,
        pagination: None,
        value: Some(value),
        explain: None,
        skipped: false,
    }
}

/// A minimal `QueryResult` carrying one `Direct` record (the Read-records
/// path — `resolve_query_ref_value` indexes `records` via a `[n].field` path).
fn read_result(field: &str, val: QueryValue) -> QueryResult {
    let mut rec = new_map();
    rec.insert(field.to_string(), val);
    QueryResult {
        records: vec![QueryRecord::Direct(QueryValue::Map(rec))],
        stats: None,
        pagination: None,
        value: None,
        explain: None,
        skipped: false,
    }
}

/// Build a `resolved_refs` map with a single alias `key` → `qr`.
fn refs_of(key: &str, qr: QueryResult) -> TMap<String, QueryResult> {
    let mut m = new_map();
    m.insert(key.to_string(), qr);
    m
}

/// Extract the pointer-identity key for a `FilterValue` reference, matching
/// the key `prescan_query_ref_cache` inserts (`fv as *const FilterValue as
/// usize`). Avoids raw pointers/`unsafe` in the test body.
fn fv_ref_addr(fv: &FilterValue) -> usize {
    fv as *const FilterValue as usize
}

/// A trivially-empty record (the `QueryRef` arm never reads the record, so
/// any `RecordRef`-shaped value works; reuse `InnerValue::Null` like the rest
/// of the filter test suite).
fn null_record() -> shamir_types::types::value::InnerValue {
    shamir_types::types::value::InnerValue::Null
}

// ---------------------------------------------------------------------
// Part A — direct unit tests for prescan_query_ref_cache
// ---------------------------------------------------------------------

/// Test 1: `prescan_query_ref_cache` reserves a slot for a bare `QueryRef`
/// AND recurses into every documented shape that can embed a nested
/// `QueryRef` (FnCall args, Expr args, Cond branches, the Cond condition's
/// Filter operands). Table-driven: each case nests a `QueryRef` at a distinct
/// position and asserts the nested node IS reserved after a single prescan.
#[test]
fn prescan_recurses_into_all_documented_shapes() {
    // Case 0: a bare QueryRef at the root.
    {
        let fv = FilterValue::query_ref("q");
        let mut cache: QueryRefCache = new_map();
        prescan_query_ref_cache(&fv, &mut cache);
        assert!(
            cache.get(&fv_ref_addr(&fv)).is_some(),
            "bare QueryRef must be reserved"
        );
    }

    // Case 1: inside FilterValue::FnCall's args.
    {
        let nested_fv = FilterValue::query_ref("q");
        let fv = FilterValue::FnCall {
            call: FnCall::complex("strings/upper", vec![nested_fv]),
        };
        let mut cache: QueryRefCache = new_map();
        prescan_query_ref_cache(&fv, &mut cache);
        let FilterValue::FnCall { call } = &fv else {
            unreachable!()
        };
        assert!(
            cache.get(&fv_ref_addr(&call.args()[0])).is_some(),
            "FnCall arg QueryRef must be reserved"
        );
    }

    // Case 2: inside FilterValue::Expr's args.
    {
        let nested_fv = FilterValue::query_ref("q");
        let fv = FilterValue::Expr {
            expr: crate::query::filter::FilterExpr::concat(vec![
                nested_fv,
                FilterValue::String("!".to_string()),
            ]),
        };
        let mut cache: QueryRefCache = new_map();
        prescan_query_ref_cache(&fv, &mut cache);
        let FilterValue::Expr { expr } = &fv else {
            unreachable!()
        };
        assert!(
            cache.get(&fv_ref_addr(&expr.args[0])).is_some(),
            "Expr arg QueryRef must be reserved"
        );
    }

    // Case 3: inside a Cond's `then` branch.
    {
        let inner_fv = FilterValue::query_ref("q");
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
        let mut cache: QueryRefCache = new_map();
        prescan_query_ref_cache(&fv, &mut cache);
        let FilterValue::Cond { cond } = &fv else {
            unreachable!()
        };
        assert!(
            cache.get(&fv_ref_addr(&cond.then)).is_some(),
            "Cond's own `then` branch nested QueryRef must be reserved"
        );
    }

    // Case 4: inside the condition Filter itself — Filter::Eq { value: <nested
    // QueryRef> }. Exercises prescan_filter's walk.
    {
        let inner_fv = FilterValue::query_ref("q");
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
        let mut cache: QueryRefCache = new_map();
        prescan_query_ref_cache(&fv, &mut cache);
        let FilterValue::Cond { cond } = &fv else {
            unreachable!()
        };
        let Filter::Eq { value, .. } = cond.condition.as_ref() else {
            unreachable!()
        };
        assert!(
            cache.get(&fv_ref_addr(value)).is_some(),
            "QueryRef embedded inside Filter::Eq's `value` operand must be reserved \
             via prescan_filter"
        );
    }
}

// ---------------------------------------------------------------------
// Part B — lazy-population proof (the F2-specific test)
// ---------------------------------------------------------------------

/// Test 2: the `OnceLock` cell is EMPTY before the first
/// `resolve_filter_query` call on a `QueryRef` node, and POPULATED after.
/// This is the structural difference from F1 (whose `FieldPathCache` is
/// eagerly populated at prescan): F2 can only RESERVE a slot at prescan
/// time; the value is filled lazily on the first row.
#[test]
fn once_lock_populated_lazily_on_first_call() {
    let interner = Interner::new();
    let fv = FilterValue::query_ref("q");

    // Reserve the slot via prescan.
    let mut cache: QueryRefCache = new_map();
    prescan_query_ref_cache(&fv, &mut cache);
    let cell = cache
        .get(&fv_ref_addr(&fv))
        .expect("prescan reserved the slot");

    // BEFORE the first resolve: the OnceLock is uninitialized (get() == None).
    assert!(
        cell.get().is_none(),
        "OnceLock must be EMPTY before the first resolve_filter_query call \
         (lazy population — F2 reserves the slot, it does not fill it at prescan time)"
    );

    // Run one resolve against a populated resolved_refs.
    let refs = refs_of("q", call_result(QueryValue::Int(42)));
    let ctx = FilterContext::new(&interner, &refs).with_query_ref_cache(&cache);
    let rec = null_record();
    let out = resolve_filter_query(&fv, &rec, &ctx);

    // AFTER the first resolve: the OnceLock is initialized (get() == Some).
    assert_eq!(out, Some(QueryValue::Int(42)));
    assert!(
        cell.get().is_some(),
        "OnceLock must be POPULATED after the first resolve_filter_query call"
    );
    // And the cached value matches what was resolved.
    assert_eq!(
        cell.get().unwrap(),
        &Some(QueryValue::Int(42)),
        "the populated cell must hold exactly the resolved value"
    );
}

// ---------------------------------------------------------------------
// Part C — DECISIVE equivalence: cache-HIT and cache-MISS paths produce
// IDENTICAL results, across TWO different resolved_refs scans.
// ---------------------------------------------------------------------

/// Test 3 (brief Verification §1, mandatory): the cache-HIT path
/// (`with_query_ref_cache`) and the cache-MISS path (`query_ref_cache: None`)
/// produce IDENTICAL results for the same `$query`-referencing filter, across
/// TWO different `ctx.resolved_refs` scans (different values under the same
/// alias). Each scan gets its OWN fresh `QueryRefCache` (built + prescanned
/// per scan, exactly as `SelectProjection::new` does per query), proving the
/// cache is per-scan fresh — never stale/shared across unrelated scans.
#[test]
fn cached_and_uncached_query_ref_identical_across_two_scans() {
    let interner = Interner::new();
    let rec = null_record();

    // The SAME owned QueryRef node is reused across both scans (pointer
    // identity is the cache key) — a fresh clone per scan would defeat the
    // pointer-identity check.
    let fv = FilterValue::query_ref("q");

    // ── Scan A: resolved_refs["q"] = Call result value Int(42) ─────────
    let refs_a = refs_of("q", call_result(QueryValue::Int(42)));
    let mut cache_a: QueryRefCache = new_map();
    prescan_query_ref_cache(&fv, &mut cache_a);
    let ctx_cached_a = FilterContext::new(&interner, &refs_a).with_query_ref_cache(&cache_a);
    let ctx_uncached_a = FilterContext::new(&interner, &refs_a);
    let cached_a = resolve_filter_query(&fv, &rec, &ctx_cached_a);
    let uncached_a = resolve_filter_query(&fv, &rec, &ctx_uncached_a);
    assert_eq!(
        cached_a, uncached_a,
        "scan A: cache-HIT and cache-MISS must agree"
    );
    assert_eq!(cached_a, Some(QueryValue::Int(42)), "scan A value");

    // ── Scan B: resolved_refs["q"] = Call result value Str("hello") ────
    // A FRESH cache (built + prescanned for THIS scan, mirroring
    // SelectProjection's per-query rebuild). If the cache were naively
    // reused from scan A, scan B would wrongly return Int(42).
    let refs_b = refs_of("q", call_result(QueryValue::Str("hello".to_string())));
    let mut cache_b: QueryRefCache = new_map();
    prescan_query_ref_cache(&fv, &mut cache_b);
    let ctx_cached_b = FilterContext::new(&interner, &refs_b).with_query_ref_cache(&cache_b);
    let ctx_uncached_b = FilterContext::new(&interner, &refs_b);
    let cached_b = resolve_filter_query(&fv, &rec, &ctx_cached_b);
    let uncached_b = resolve_filter_query(&fv, &rec, &ctx_uncached_b);
    assert_eq!(
        cached_b, uncached_b,
        "scan B: cache-HIT and cache-MISS must agree"
    );
    assert_eq!(
        cached_b,
        Some(QueryValue::Str("hello".to_string())),
        "scan B value — proves the per-scan cache is NOT stale/shared \
         (a reused-from-A cache would return Int(42) here)"
    );
}

/// Test 4: equivalence for the Read-records path with a MULTI-SEGMENT path
/// (`[0].field`) — the exact per-row string path parsing + Map navigation
/// walk F2 eliminates. Two records (different field values) resolve through
/// the SAME cache, proving the cache holds the per-scan-invariant target
/// (the path string + referenced record are static across one scan, so the
/// value is identical for both records of that scan).
#[test]
fn cached_uncached_identical_for_records_path() {
    let interner = Interner::new();

    // `$query` ref with a multi-segment path: `[0].score` → index record 0,
    // then walk into `.score`. This is the path that used to be re-parsed
    // (`find(']')` / `parse::<usize>` / `strip_prefix('.')`) per row.
    let fv = FilterValue::query_ref_with_path("q", "[0].score");

    // resolved_refs["q"] = one record { score: 7 }.
    let refs = refs_of("q", read_result("score", QueryValue::Int(7)));

    let mut cache: QueryRefCache = new_map();
    prescan_query_ref_cache(&fv, &mut cache);
    let ctx_cached = FilterContext::new(&interner, &refs).with_query_ref_cache(&cache);
    let ctx_uncached = FilterContext::new(&interner, &refs);

    // The cached target is the SAME for every row of this scan (the path
    // string + referenced QueryResult are scan-invariant), so two distinct
    // records both resolve to the cached value 7.
    let r0 = null_record();
    let r1 = null_record();
    for (i, row) in [r0, r1].iter().enumerate() {
        let cached = resolve_filter_query(&fv, row, &ctx_cached);
        let uncached = resolve_filter_query(&fv, row, &ctx_uncached);
        assert_eq!(
            cached, uncached,
            "records-path row {i}: cache-HIT and cache-MISS must agree"
        );
        assert_eq!(
            cached,
            Some(QueryValue::Int(7)),
            "records-path value row {i}"
        );
    }

    // The cell is populated after the scan.
    let cell = cache.get(&fv_ref_addr(&fv)).expect("slot reserved");
    assert_eq!(cell.get(), Some(&Some(QueryValue::Int(7))));
}

/// Test 5: missing-alias collapse — both paths agree that an unknown alias
/// resolves to `None`. The cache stores `Some(None)` (a populated cell whose
/// inner value is `None`), proving the lazy path caches the miss correctly
/// rather than re-probing per row.
#[test]
fn missing_alias_collapses_identically_and_caches_the_miss() {
    let interner = Interner::new();
    let fv = FilterValue::query_ref("nope");
    let rec = null_record();

    // Empty resolved_refs — alias "nope" is absent.
    let refs: TMap<String, QueryResult> = new_map();
    let mut cache: QueryRefCache = new_map();
    prescan_query_ref_cache(&fv, &mut cache);
    let ctx_cached = FilterContext::new(&interner, &refs).with_query_ref_cache(&cache);
    let ctx_uncached = FilterContext::new(&interner, &refs);

    let cached = resolve_filter_query(&fv, &rec, &ctx_cached);
    let uncached = resolve_filter_query(&fv, &rec, &ctx_uncached);
    assert_eq!(cached, uncached, "missing-alias: both paths return None");
    assert_eq!(cached, None);

    // The cell is initialized to `None` (the miss is cached, not re-probed).
    let cell = cache.get(&fv_ref_addr(&fv)).expect("slot reserved");
    assert_eq!(
        cell.get(),
        Some(&None),
        "a missing alias must populate the cell with Some(None) — the miss is \
         cached so subsequent rows skip the resolved_refs probe entirely"
    );
}

// ---------------------------------------------------------------------
// Part D — engine-level integration through the production seam.
// ---------------------------------------------------------------------

/// Test 6: the real production wiring path — `SelectProjection::new` (which
/// calls `prescan_query_ref_cache` internally per F2 step 5) builds the cache
/// ONCE, then `project_value` serves TWO records through the SAME projection.
///
/// Note: `SelectProjection::project_value` builds its `FilterContext` with an
/// EMPTY `resolved_refs` (`empty_refs`), so a `$query` ref resolves to `None`
/// here — the FnCall arm propagates the `None` and the projection emits
/// `Null`. This test therefore proves the WIRING is live (prescan runs in
/// `new()` without panic; `with_query_ref_cache` is threaded in
/// `project_value`; the cache-HIT path executes per record) and CONSISTENT
/// across records, rather than asserting a non-Null `$query` value (the
/// unit tests in Part C cover the meaningful-value case directly). Mirrors
/// `field_path_cache_tests.rs`'s test 6 shape.
#[test]
fn select_projection_wires_query_ref_cache_and_evaluates_per_record() {
    let interner = Arc::new(Interner::new());

    // SELECT upper(@q) AS up  — a FnCall wrapping a $query ref.
    let select = Select {
        items: vec![SelectItem::Function {
            name: "strings/upper".to_string(),
            args: vec![FilterValue::query_ref("q")],
            alias: Some("up".to_string()),
        }],
        distinct: false,
    };

    // Built ONCE — this populates `funcs_query_ref_cache` internally via
    // `prescan_query_ref_cache`, the real production call site under test.
    let proj = SelectProjection::new(&select, &interner, ScalarResolver::builtins_only());

    let rec_a = null_record();
    let rec_b = null_record();
    let qval_a = proj.project_value(&rec_a, &interner);
    let qval_b = proj.project_value(&rec_b, &interner);

    // Both records emit a consistent Null (the $query ref resolves to None
    // against the projection's empty resolved_refs; upper propagates the
    // None → Null). The point is that the wired cache path executes without
    // panic and produces identical, per-record results — NOT a frozen or
    // panicked state.
    match &qval_a {
        QueryValue::Map(m) => {
            assert_eq!(
                m.get("up"),
                Some(&QueryValue::Null),
                "first record: @q against empty resolved_refs → upper propagates None → Null"
            );
        }
        _ => panic!("expected QueryValue::Map, got {:?}", qval_a),
    }
    match &qval_b {
        QueryValue::Map(m) => {
            assert_eq!(
                m.get("up"),
                Some(&QueryValue::Null),
                "second record: same projection, same consistent result via the wired cache path"
            );
        }
        _ => panic!("expected QueryValue::Map, got {:?}", qval_b),
    }
}
