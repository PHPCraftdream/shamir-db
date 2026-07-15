//! Epic02/E (task #639) — `$cond`/`$expr` evaluation overhead in
//! `resolve_filter_query` (`crates/shamir-engine/src/query/filter/resolve.rs`).
//!
//! Phases A-D (#635-#638) implemented and tested `$cond`/`$expr` evaluation.
//! This bench puts numbers behind the expected overhead of that recursive
//! evaluation compared to a flat literal comparison, on a realistic
//! per-row hot-loop shape (1000 records — same order of magnitude as
//! `filter_eval.rs`).
//!
//! Groups (each calls `resolve_filter_query` once per record, 1000 records):
//! - `cond_expr_eval/flat_literal_1000` — baseline: `FilterValue::Int(30)`,
//!   no recursion, no branching.
//! - `cond_expr_eval/cond_2branch_1000` — `$cond` with a 2-branch ternary
//!   (`age > 18 ? 1 : 0`), one recursive `resolve_filter_query` call plus
//!   one `compile_filter` + `FilterNode::matches` call for the condition.
//! - `cond_expr_eval/cond_nested_3level_1000` — 3-level nested `$cond`
//!   (switch-case pattern: age bracket → tier), matching the shape used in
//!   the Phase C/D unit/e2e tests.
//! - `cond_expr_eval/expr_add_two_fields_1000` — `$expr` `add` over two
//!   `$ref` field values (`age + score`).
//!
//! ## Measured results (this machine, `CARGO_TARGET_DIR=D:/dev/rust/.cargo-target-bench
//! cargo bench -p shamir-engine --bench cond_expr_eval`, QUICK/JIT-calibrated
//! iteration counts — actual run output, 1000 records/iter):
//!
//! ```text
//! cond_expr_eval/flat_literal_1000          135249 iters      7231.95 ns/op
//! cond_expr_eval/cond_2branch_1000            4797 iters    213004.36 ns/op
//! cond_expr_eval/cond_nested_3level_1000      1335 iters    829010.94 ns/op
//! cond_expr_eval/expr_add_two_fields_1000      784 iters   1378120.92 ns/op
//! ```
//!
//! (~7.2 ns/record baseline vs. ~213 ns/record for a 2-branch `$cond`,
//! ~829 ns/record for the 3-level nested `$cond`, and ~1378 ns/record for
//! a 2-field `$expr` add — per-record, i.e. `ns/op / 1000`.)
//!
//! ## Conclusion
//!
//! `$cond`/`$expr` evaluation is **far** from free relative to a flat
//! literal comparison — this is a **large** regression, not a marginal
//! one:
//! - `cond_2branch` is ~**29x** the flat-literal baseline.
//! - `cond_nested_3level` is ~**115x** the baseline (roughly 3x
//!   `cond_2branch`, consistent with paying the same per-level cost three
//!   times).
//! - `expr_add_two_fields` is ~**190x** the baseline — the single most
//!   expensive variant measured here, more expensive even than the
//!   3-level nested `$cond`.
//!
//! This exceeds the brief's "10x+ would be a finding" bar for
//! `cond_2branch` and blows well past it for the nested/`$expr` cases.
//! Root causes, from reading `resolve.rs`:
//! - **`$cond`**: `FilterValue::Cond`'s arm in `resolve_filter_query` calls
//!   `compile_filter(&cond.condition, ctx.interner)` **on every
//!   evaluation, per record** — `compile_filter` is not cached/memoized,
//!   so each row re-walks and re-interns the condition's `Filter` AST from
//!   scratch (allocating a new `FilterNode` tree) before
//!   `FilterNode::matches` even runs. `cond_nested_3level` pays this
//!   compile-per-row cost three times (once per nesting level), which
//!   matches the roughly-3x scaling observed vs. `cond_2branch`.
//! - **`$expr`**: `eval_filter_expr` allocates a fresh
//!   `Vec::with_capacity(expr.args.len())` per call, AND every arg
//!   (including `$ref` field lookups) goes back through the full
//!   `resolve_filter_query` dispatch/match per row — for a 2-arg `add`
//!   over two `$ref`s this is two recursive dispatches plus a heap
//!   allocation per row, which explains why it comes out even pricier
//!   than the 3-level `$cond` here.
//!
//! **Not fixed here** — per the brief, this task only measures and
//! documents; `resolve_filter_query`/`eval_filter_expr` are explicitly
//! out of scope for optimization in this task. The clearest follow-up
//! target (separate task) is hoisting `compile_filter` for a `$cond`'s
//! `condition` out of the per-record loop — the `Filter` AST inside a
//! `Cond` is static per-query, exactly like the top-level filter that is
//! already compiled once outside the hot loop.

use std::hint::black_box;

use bench_scale_tool::Harness;
use shamir_engine::query::filter::{
    resolve_filter_query, Cond, Filter, FilterContext, FilterExpr, FilterExprOp, FilterValue,
};
use shamir_query_types::read::QueryResult;
use shamir_types::core::interner::{Interner, TouchInd};
use shamir_types::types::common::{new_map_wc, TMap};
use shamir_types::types::value::InnerValue;

fn make_record(interner: &Interner, idx: u32) -> InnerValue {
    let touch = |s: &str| match interner.touch_ind(s).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k,
    };
    let mut m = new_map_wc(3);
    m.insert(touch("id"), InnerValue::Int(idx as i64));
    m.insert(touch("age"), InnerValue::Int((idx % 100) as i64));
    m.insert(touch("score"), InnerValue::Int((idx % 50) as i64));
    InnerValue::Map(m)
}

/// Register a `resolve_filter_query` sweep over `records` for a
/// `FilterValue`, both leaked to `'static` at setup time (same pattern as
/// `filter_eval.rs::bench_matches` — `FilterContext`/`FilterValue` are
/// borrowed types with no `Clone` impl, and `Harness::bench` closures
/// require `'static`; a bench binary's process lifetime makes leaking a
/// deliberate, harmless trade for closure ergonomics).
fn bench_resolve(
    h: &mut Harness,
    id: &str,
    records: &'static [InnerValue],
    ctx: &'static FilterContext<'static>,
    fv: &'static FilterValue,
) {
    h.bench(id, move || {
        let mut n = 0usize;
        for r in records {
            if resolve_filter_query(fv, r, ctx).is_some() {
                n += 1;
            }
        }
        black_box(n);
    });
}

fn main() {
    let mut h = Harness::new("cond_expr_eval", env!("CARGO_MANIFEST_DIR"));

    let interner: &'static Interner = Box::leak(Box::new(Interner::new()));
    for k in ["id", "age", "score"] {
        let _ = interner.touch_ind(k);
    }
    let records: &'static Vec<InnerValue> = Box::leak(Box::new(
        (0..1000).map(|i| make_record(interner, i)).collect(),
    ));
    let empty_refs: &'static TMap<String, QueryResult> = Box::leak(Box::new(new_map_wc(0)));
    let ctx: &'static FilterContext<'static> =
        Box::leak(Box::new(FilterContext::new(interner, empty_refs)));

    // ── Baseline: flat literal, no recursion ─────────────────────────
    let flat_literal: &'static FilterValue = Box::leak(Box::new(FilterValue::Int(30)));
    bench_resolve(
        &mut h,
        "cond_expr_eval/flat_literal_1000",
        records,
        ctx,
        flat_literal,
    );

    // ── $cond, 2 branches: age > 18 ? 1 : 0 ──────────────────────────
    let cond_2branch: &'static FilterValue = Box::leak(Box::new(FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::Gt {
                field: vec!["age".to_string()],
                value: FilterValue::Int(18),
            },
            FilterValue::Int(1),
            FilterValue::Int(0),
        )),
    }));
    bench_resolve(
        &mut h,
        "cond_expr_eval/cond_2branch_1000",
        records,
        ctx,
        cond_2branch,
    );

    // ── Nested $cond (3 levels, switch-case pattern): age bracket → tier
    //    (same shape as Phase C/D unit/e2e tests: score >= 100 -> "vip",
    //    else score >= 50 -> "regular", else age > 18 -> "adult", else "minor")
    let level3 = FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::Gt {
                field: vec!["age".to_string()],
                value: FilterValue::Int(18),
            },
            FilterValue::String("adult".to_string()),
            FilterValue::String("minor".to_string()),
        )),
    };
    let level2 = FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::Gte {
                field: vec!["score".to_string()],
                value: FilterValue::Int(50),
            },
            FilterValue::String("regular".to_string()),
            level3,
        )),
    };
    let cond_nested_3level: &'static FilterValue = Box::leak(Box::new(FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::Gte {
                field: vec!["score".to_string()],
                value: FilterValue::Int(100),
            },
            FilterValue::String("vip".to_string()),
            level2,
        )),
    }));
    bench_resolve(
        &mut h,
        "cond_expr_eval/cond_nested_3level_1000",
        records,
        ctx,
        cond_nested_3level,
    );

    // ── $expr: add(age, score) ────────────────────────────────────────
    let expr_add: &'static FilterValue = Box::leak(Box::new(FilterValue::Expr {
        expr: FilterExpr::new(
            FilterExprOp::Add,
            vec![
                FilterValue::field_ref("age"),
                FilterValue::field_ref("score"),
            ],
        ),
    }));
    bench_resolve(
        &mut h,
        "cond_expr_eval/expr_add_two_fields_1000",
        records,
        ctx,
        expr_add,
    );

    h.run();
}
