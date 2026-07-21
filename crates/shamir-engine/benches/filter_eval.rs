//! Hot-loop bench for filter `matches()` on 1000 records.
//!
//! Each callback walks the record via `resolve_field` for each path. The
//! baseline implementation returns `Option<InnerValue>`, cloning the
//! resolved value before the comparison even runs. The intended
//! optimisation returns a borrow (`Option<&InnerValue>`), avoiding the
//! clone entirely on a path that runs once per record per predicate.
//!
//! Migrated to the fixed-iteration harness (`bench_scale_tool`): setup
//! (interner, records, compiled filters) is built ONCE outside the timed
//! closure, exactly as under Criterion's `b.iter` — plan 1 (shared setup).
//!
//! `Interner` / `FilterNode` / `FilterContext` are borrowed types with no
//! `Clone` impl, and the harness's `bench()` closures require `'static`.
//! Rather than fight the borrow checker with `Rc` plumbing through every
//! callback, the (small, one-shot) setup fixtures are leaked to `'static`
//! via `Box::leak` — a bench binary's process lifetime makes this a
//! deliberate, harmless trade for closure ergonomics.

use std::hint::black_box;

use bench_scale_tool::Harness;
use shamir_engine::query::filter::Filter;
use shamir_engine::query::filter::{compile_filter, FilterContext, FilterNode};
use shamir_query_types::filter::FilterValue;
use shamir_query_types::read::{QueryRecord, QueryResult};

use shamir_types::core::interner::{Interner, TouchInd};
use shamir_types::mpack;
use shamir_types::types::common::{new_map_wc, TMap};
use shamir_types::types::value::{InnerValue, Value};

fn intern(i: &Interner, s: &str) {
    let _ = i.touch_ind(s);
}

fn make_record(interner: &Interner, idx: u32) -> InnerValue {
    let touch = |s: &str| match interner.touch_ind(s).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k,
    };
    let mut m = new_map_wc(10);
    m.insert(touch("id"), InnerValue::Int(idx as i64));
    m.insert(touch("name"), InnerValue::Str(format!("user-{}", idx)));
    m.insert(touch("age"), InnerValue::Int((idx % 100) as i64));
    m.insert(touch("score"), InnerValue::F64(idx as f64 * 1.5));
    m.insert(touch("active"), InnerValue::Bool(idx.is_multiple_of(2)));
    m.insert(
        touch("email"),
        InnerValue::Str(format!("u{}@example.com", idx)),
    );
    m.insert(touch("tags"), {
        InnerValue::List(vec![
            InnerValue::Str("alpha".into()),
            InnerValue::Str("beta".into()),
            InnerValue::Str("gamma".into()),
        ])
    });
    m.insert(touch("address"), {
        let mut a = new_map_wc(3);
        a.insert(touch("city"), InnerValue::Str("Jerusalem".into()));
        a.insert(touch("zip"), InnerValue::Str("9100000".into()));
        a.insert(touch("country"), InnerValue::Str("IL".into()));
        InnerValue::Map(a)
    });
    InnerValue::Map(m)
}

/// Register a `matches()` sweep over `records` for a compiled filter,
/// both leaked to `'static` at setup time.
fn bench_matches(
    h: &mut Harness,
    id: &str,
    records: &'static [InnerValue],
    ctx: &'static FilterContext<'static>,
    filter: &'static FilterNode,
) {
    h.bench(id, move || {
        let mut n = 0usize;
        for r in records {
            if filter.matches(r, ctx) {
                n += 1;
            }
        }
        black_box(n);
    });
}

fn main() {
    let mut h = Harness::new("filter_eval", env!("CARGO_MANIFEST_DIR"));

    let interner: &'static Interner = Box::leak(Box::new(Interner::new()));
    // Make sure all field-names are interned before compile_filter runs.
    for k in [
        "id", "name", "age", "score", "active", "email", "tags", "address", "city", "zip",
        "country",
    ] {
        intern(interner, k);
    }
    let records: &'static Vec<InnerValue> = Box::leak(Box::new(
        (0..1000).map(|i| make_record(interner, i)).collect(),
    ));
    let empty_refs: &'static TMap<String, QueryResult> = Box::leak(Box::new(new_map_wc(0)));
    let ctx: &'static FilterContext<'static> =
        Box::leak(Box::new(FilterContext::new(interner, empty_refs)));

    let eq_age: &'static FilterNode = Box::leak(Box::new(compile_filter(
        &Filter::Eq {
            field: vec!["age".to_string()],
            value: FilterValue::Int(50),
        },
        interner,
    )));
    bench_matches(
        &mut h,
        "filter_eval/eq_int_top_level_1000",
        records,
        ctx,
        eq_age,
    );

    let eq_nested: &'static FilterNode = Box::leak(Box::new(compile_filter(
        &Filter::Eq {
            field: vec!["address".to_string(), "city".to_string()],
            value: FilterValue::String("Jerusalem".to_string()),
        },
        interner,
    )));
    bench_matches(
        &mut h,
        "filter_eval/eq_str_nested_path_1000",
        records,
        ctx,
        eq_nested,
    );

    // ── Compound AND: age > 20 AND active = true ──────────────
    let compound_and: &'static FilterNode = Box::leak(Box::new(compile_filter(
        &Filter::And {
            filters: vec![
                Filter::Gt {
                    field: vec!["age".to_string()],
                    value: FilterValue::Int(20),
                },
                Filter::Eq {
                    field: vec!["active".to_string()],
                    value: FilterValue::Bool(true),
                },
            ],
        },
        interner,
    )));
    bench_matches(
        &mut h,
        "filter_eval/compound_and_2_1000",
        records,
        ctx,
        compound_and,
    );

    // ── Compound AND(3): age > 20 AND active = true AND score < 500 ─
    let compound_and3: &'static FilterNode = Box::leak(Box::new(compile_filter(
        &Filter::And {
            filters: vec![
                Filter::Gt {
                    field: vec!["age".to_string()],
                    value: FilterValue::Int(20),
                },
                Filter::Eq {
                    field: vec!["active".to_string()],
                    value: FilterValue::Bool(true),
                },
                Filter::Lt {
                    field: vec!["score".to_string()],
                    value: FilterValue::Float(500.0),
                },
            ],
        },
        interner,
    )));
    bench_matches(
        &mut h,
        "filter_eval/compound_and_3_1000",
        records,
        ctx,
        compound_and3,
    );

    // ── Compound OR: age = 50 OR age = 30 ──────────────────────
    let compound_or: &'static FilterNode = Box::leak(Box::new(compile_filter(
        &Filter::Or {
            filters: vec![
                Filter::Eq {
                    field: vec!["age".to_string()],
                    value: FilterValue::Int(50),
                },
                Filter::Eq {
                    field: vec!["age".to_string()],
                    value: FilterValue::Int(30),
                },
            ],
        },
        interner,
    )));
    bench_matches(
        &mut h,
        "filter_eval/compound_or_2_1000",
        records,
        ctx,
        compound_or,
    );

    // ── Regex match on name ────────────────────────────────────
    let regex_filter: &'static FilterNode = Box::leak(Box::new(compile_filter(
        &Filter::Regex {
            field: vec!["name".to_string()],
            pattern: "user-[0-9]{2}$".to_string(),
        },
        interner,
    )));
    bench_matches(
        &mut h,
        "filter_eval/regex_match_1000",
        records,
        ctx,
        regex_filter,
    );

    // ── FTS brute-force AND: 2 tokens on 1000 text records ──────
    let fts_and: &'static FilterNode = Box::leak(Box::new(compile_filter(
        &Filter::Fts {
            field: vec!["name".to_string()],
            query: "user alpha".to_string(),
            mode: "and".to_string(),
        },
        interner,
    )));
    bench_matches(
        &mut h,
        "filter_eval/fts_brute_and_1000",
        records,
        ctx,
        fts_and,
    );

    // ── FTS brute-force OR: 2 tokens on 1000 text records ────
    let fts_or: &'static FilterNode = Box::leak(Box::new(compile_filter(
        &Filter::Fts {
            field: vec!["name".to_string()],
            query: "user alpha".to_string(),
            mode: "or".to_string(),
        },
        interner,
    )));
    bench_matches(
        &mut h,
        "filter_eval/fts_brute_or_1000",
        records,
        ctx,
        fts_or,
    );

    // ── IN list: int membership over a medium-sized list ─────
    let in_int_32: &'static FilterNode = Box::leak(Box::new(compile_filter(
        &Filter::In {
            field: vec!["age".to_string()],
            values: (0i64..32).map(FilterValue::Int).collect(),
        },
        interner,
    )));
    bench_matches(
        &mut h,
        "filter_eval/in_int_list_32_1000",
        records,
        ctx,
        in_int_32,
    );

    // ── IN list: string membership over a medium-sized list ─────
    let in_str_32: &'static FilterNode = Box::leak(Box::new(compile_filter(
        &Filter::In {
            field: vec!["name".to_string()],
            values: (0..32)
                .map(|i| FilterValue::String(format!("user-{}", i * 7)))
                .collect(),
        },
        interner,
    )));
    bench_matches(
        &mut h,
        "filter_eval/in_str_list_32_1000",
        records,
        ctx,
        in_str_32,
    );

    // ── Computed: LOWER(name) == "user-50" on 1000 records ────
    let computed_lower: &'static FilterNode = Box::leak(Box::new(compile_filter(
        &Filter::Computed {
            expr_op: "lower".to_string(),
            field: vec!["name".to_string()],
            expr_args: None,
            cmp: "eq".to_string(),
            value: FilterValue::String("user-50".to_string()),
        },
        interner,
    )));
    bench_matches(
        &mut h,
        "filter_eval/computed_lower_eq_1000",
        records,
        ctx,
        computed_lower,
    );

    // ============================================================================
    // $in @ref column — scaling bench (O(N²) → O(N) target).
    //
    // A `$in @ref[].field` filter rebuilds the entire ref-column AND linear-scans
    // it ONCE PER outer record. With M outer rows and K ref rows that's O(M·K).
    // This group measures the per-N wall time to make quadratic growth visible
    // and verify the curve stays flat after the memoisation fix — N=100 alone
    // cannot show that shape, so the sweep is a genuine algorithmic-scaling
    // demonstration, not a cheap-call-repeated-N-times artifact. Default sweep
    // keeps only N=100 (~12µs/call); N=1000/10000 (the tiers that actually
    // reveal the O(M·K) curve) are opt-in via BENCH_FILTER_EVAL_SCALING=1 so
    // the fast default sweep isn't stuck with a multi-ms call.
    // ============================================================================
    let bench_scaling_tail = std::env::var("BENCH_FILTER_EVAL_SCALING")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);
    let interner2: &'static Interner = Box::leak(Box::new(Interner::new()));
    for k in [
        "id", "name", "age", "score", "active", "email", "tags", "val",
    ] {
        let _ = interner2.touch_ind(k);
    }

    const REF_ROWS: usize = 100;
    let ref_records: Vec<QueryRecord> = (0..REF_ROWS)
        .map(|i| QueryRecord::Direct(mpack!({"val": @ Value::Int(i as i64)})))
        .collect();
    let mut refs: TMap<String, QueryResult> = new_map_wc(1);
    refs.insert(
        "ref".to_string(),
        QueryResult {
            records: ref_records,
            stats: None,
            pagination: None,
            value: None,
            explain: None,
            skipped: false,
            versions: None,
        },
    );
    let refs: &'static TMap<String, QueryResult> = Box::leak(Box::new(refs));
    let ctx2: &'static FilterContext<'static> =
        Box::leak(Box::new(FilterContext::new(interner2, refs)));

    let in_ref: &'static FilterNode = Box::leak(Box::new(compile_filter(
        &Filter::In {
            field: vec!["age".to_string()],
            values: vec![FilterValue::QueryRef {
                alias: "ref".to_string(),
                path: Some("[].val".to_string()),
            }],
        },
        interner2,
    )));

    let mut in_ref_ns = vec![100usize];
    if bench_scaling_tail {
        in_ref_ns.push(1_000);
        in_ref_ns.push(10_000);
    }
    for n in in_ref_ns {
        let records: &'static Vec<InnerValue> = Box::leak(Box::new(
            (0..n)
                .map(|i| {
                    let touch = |s: &str| match interner2.touch_ind(s).unwrap() {
                        TouchInd::Exists(k) | TouchInd::New(k) => k,
                    };
                    let mut m = new_map_wc(2);
                    m.insert(touch("id"), InnerValue::Int(i as i64));
                    m.insert(touch("age"), InnerValue::Int((i % REF_ROWS) as i64));
                    InnerValue::Map(m)
                })
                .collect(),
        ));

        bench_matches(
            &mut h,
            &format!("filter_in_query_ref_scaling/in_at_ref_col_{n}"),
            records,
            ctx2,
            in_ref,
        );
    }

    h.run();
}
